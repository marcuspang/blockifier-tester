use std::{fmt::Display, fs, path::Path};

use futures::future::join_all;
use itertools::Itertools;
use starknet::{
    core::types::{
        BlockId, BroadcastedDeployAccountTransaction, BroadcastedDeployAccountTransactionV1,
        BroadcastedDeployAccountTransactionV3, BroadcastedInvokeTransaction,
        BroadcastedInvokeTransactionV1, BroadcastedInvokeTransactionV3, BroadcastedTransaction,
        DeployAccountTransaction, ExecuteInvocation, ExecutionResult, FieldElement,
        InvokeTransaction, MaybePendingBlockWithTxs, MaybePendingTransactionReceipt,
        SimulatedTransaction, Transaction, TransactionTrace,
    },
    providers::{Provider, ProviderError},
};

use crate::juno_manager::{JunoManager, ManagerError};

trait TransactionSimulator {
    async fn get_expected_transaction_result(
        &self,
        tx_hash: FieldElement,
    ) -> Result<TransactionResult, ProviderError>;
    async fn get_transactions_to_simulate(
        &mut self,
        block: &MaybePendingBlockWithTxs,
    ) -> Result<Vec<TransactionToSimulate>, ManagerError>;
    async fn simulate_block(
        &mut self,
        block_number: u64,
    ) -> Result<Vec<SimulationReport>, ManagerError>;
    async fn pessimistic_repeat_simulate_until_success(
        &mut self,
        block_id: BlockId,
        transactions: &[TransactionToSimulate],
    ) -> Result<Vec<SimulatedTransaction>, ManagerError>;
}

impl TransactionSimulator for JunoManager {
    async fn get_expected_transaction_result(
        &self,
        tx_hash: FieldElement,
    ) -> Result<TransactionResult, ProviderError> {
        self.rpc_client
            .get_transaction_receipt(tx_hash)
            .await
            .map(|receipt| receipt.into())
    }

    async fn get_transactions_to_simulate(
        &mut self,
        block: &MaybePendingBlockWithTxs,
    ) -> Result<Vec<TransactionToSimulate>, ManagerError> {
        join_all(block.transactions().iter().map(|tx| async {
            let tx_hash = get_block_transaction_hash(tx);
            self.get_expected_transaction_result(tx_hash)
                .await
                .map_err(ManagerError::from)
                .and_then(|expected_result| {
                    Ok(TransactionToSimulate {
                        tx: block_transaction_to_broadcasted_transaction(tx)?,
                        hash: tx_hash,
                        expected_result,
                    })
                })
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, ManagerError>>()
    }

    async fn simulate_block(
        &mut self,
        block_number: u64,
    ) -> Result<Vec<SimulationReport>, ManagerError> {
        println!("Getting block {block_number} with txns");
        let block = self
            .get_block_with_txs(BlockId::Number(block_number))
            .await?;

        println!("Getting transactions to simulate");
        let transactions = self.get_transactions_to_simulate(&block).await?;
        let simulation_results = self
            .pessimistic_repeat_simulate_until_success(
                BlockId::Number(block_number - 1),
                &transactions,
            )
            .await?;

        let mut found_crash = false;
        let mut report = vec![];
        for i in 0..transactions.len() {
            let tx = &transactions[i];
            let simulated_result = if i < simulation_results.len() {
                get_simulated_transaction_result(&simulation_results[i])
            } else if found_crash {
                TransactionResult::Unreached
            } else {
                found_crash = true;
                TransactionResult::Crash
            };
            report.push(SimulationReport {
                tx_hash: tx.hash,
                simulated_result,
                expected_result: tx.expected_result.clone(),
            });
        }

        Ok(report)
    }

    // Add one transaction at a time to the set that are tried
    async fn pessimistic_repeat_simulate_until_success(
        &mut self,
        block_id: BlockId,
        transactions: &[TransactionToSimulate],
    ) -> Result<Vec<SimulatedTransaction>, ManagerError> {
        let mut results = vec![];

        let broadcasted_transactions = transactions.iter().map(|tx| tx.tx.clone()).collect_vec();
        for i in 0..transactions.len() {
            let transactions_to_try = &broadcasted_transactions[0..i + 1];
            println!("Trying {} tranactions", transactions_to_try.len());
            let simulation_result = self
                .rpc_client
                .simulate_transactions(block_id, transactions_to_try, [])
                .await;

            if simulation_result.is_ok() {
                results = simulation_result.unwrap();
            } else {
                // Wait for current juno process to die so that a new one can be safely started
                self.process.wait().unwrap();
                self.process = Self::spawn_process_unchecked();
                self.ensure_usable().await?;
                return Ok(results);
            }
        }
        Ok(results)
    }
}

#[derive(Clone, Debug)]
enum TransactionResult {
    Success,
    Revert { reason: String },
    Crash,
    Unreached,

    // TEMP
    DeployAccount,
    L1Handler,
    Declare,
}

// To be used when outputting in json format
impl Display for TransactionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionResult::Success => write!(f, "Success"),
            TransactionResult::Revert { reason } => write!(f, "Reverted: {}", reason),
            TransactionResult::Crash => write!(f, "Crash"),
            TransactionResult::Unreached => write!(f, "Unreached"),
            TransactionResult::DeployAccount => {
                write!(f, "TODO determine success of deploy account transactions")
            }
            TransactionResult::L1Handler => write!(f, "L1Handler transactions not handled yet"),
            TransactionResult::Declare => write!(f, "Declare transactions not handled yet"),
        }
    }
}

impl From<MaybePendingTransactionReceipt> for TransactionResult {
    fn from(value: MaybePendingTransactionReceipt) -> Self {
        match value.execution_result() {
            ExecutionResult::Succeeded => Self::Success,
            ExecutionResult::Reverted { reason } => Self::Revert {
                reason: reason.clone(),
            },
        }
    }
}

#[derive(Debug)]
pub struct SimulationReport {
    tx_hash: FieldElement,
    expected_result: TransactionResult,
    simulated_result: TransactionResult,
}

impl Display for SimulationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{\n\"Hash\": \"{}\",\n\"Expected result\": \"{}\",\n\"Simulated result\": \"{}\"}}",
            self.tx_hash, self.expected_result, self.simulated_result
        )
    }
}

fn log_block_report(block_number: u64, report: Vec<SimulationReport>) {
    println!("Log report for block {block_number}");
    let text = report
        .iter()
        .map(|simulation_report| format!("{}", simulation_report))
        .join(",\n");
    fs::write(
        Path::new(&format!("./results/{}.json", block_number)),
        format!("{{\n\"Block number\": {block_number}\n\"Transactions\": [\n{text}]}}"),
    )
    .expect("Failed to write block report");
}

pub struct TransactionToSimulate {
    tx: BroadcastedTransaction,
    hash: FieldElement,
    expected_result: TransactionResult,
}

fn block_transaction_to_broadcasted_transaction(
    transaction: &Transaction,
) -> Result<BroadcastedTransaction, ManagerError> {
    match transaction {
        Transaction::Invoke(invoke_transaction) => match invoke_transaction {
            InvokeTransaction::V0(_) => Err(ManagerError::InternalError("V0 invoke".to_string())),
            InvokeTransaction::V1(tx) => Ok(BroadcastedTransaction::Invoke(
                BroadcastedInvokeTransaction::V1(BroadcastedInvokeTransactionV1 {
                    sender_address: tx.sender_address,
                    calldata: tx.calldata.clone(),
                    max_fee: tx.max_fee,
                    signature: tx.signature.clone(),
                    nonce: tx.nonce,
                    is_query: false,
                }),
            )),
            InvokeTransaction::V3(tx) => Ok(BroadcastedTransaction::Invoke(
                BroadcastedInvokeTransaction::V3(BroadcastedInvokeTransactionV3 {
                    sender_address: tx.sender_address,
                    calldata: tx.calldata.clone(),
                    signature: tx.signature.clone(),
                    nonce: tx.nonce,
                    resource_bounds: tx.resource_bounds.clone(),
                    tip: tx.tip,
                    paymaster_data: tx.paymaster_data.clone(),
                    account_deployment_data: tx.account_deployment_data.clone(),
                    nonce_data_availability_mode: tx.nonce_data_availability_mode,
                    fee_data_availability_mode: tx.fee_data_availability_mode,
                    is_query: false,
                }),
            )),
        },
        Transaction::L1Handler(_) => Err(ManagerError::InternalError("L1Handler".to_string())),
        Transaction::Declare(_declare_transaction) => {
            Err(ManagerError::InternalError("Declare".to_string()))
            // BroadcastedTransaction::Declare(match declare_transaction {
            //     DeclareTransaction::V0(_) => panic!("V0"),
            //     DeclareTransaction::V1(tx) => {
            //         BroadcastedDeclareTransaction::V1(BroadcastedDeclareTransactionV1 {
            //             sender_address: tx.sender_address,
            //             max_fee: tx.max_fee,
            //             signature: tx.signature.clone(),
            //             nonce: tx.nonce,
            //             contract_class: todo!("contract class"), DO NOT USE todo!
            //             is_query: false,
            //         })
            //     }
            //     DeclareTransaction::V2(_tx) => {
            //         todo!("Declare v2")
            //         // BroadcastedDeclareTransaction::V2()
            //     }
            //     DeclareTransaction::V3(_tx) => {
            //         todo!("Declare v3")
            //         // BroadcastedDeclareTransaction::V3()
            //     }
            // })
        }
        Transaction::Deploy(_) => Err(ManagerError::InternalError("Deploy".to_string())),
        Transaction::DeployAccount(tx) => Ok(BroadcastedTransaction::DeployAccount(match tx {
            DeployAccountTransaction::V1(tx) => {
                BroadcastedDeployAccountTransaction::V1(BroadcastedDeployAccountTransactionV1 {
                    max_fee: tx.max_fee,
                    signature: tx.signature.clone(),
                    nonce: tx.nonce,
                    contract_address_salt: tx.contract_address_salt,
                    constructor_calldata: tx.constructor_calldata.clone(),
                    class_hash: tx.class_hash,
                    is_query: false,
                })
            }
            DeployAccountTransaction::V3(tx) => {
                BroadcastedDeployAccountTransaction::V3(BroadcastedDeployAccountTransactionV3 {
                    signature: tx.signature.clone(),
                    nonce: tx.nonce,
                    contract_address_salt: tx.contract_address_salt,
                    constructor_calldata: tx.constructor_calldata.clone(),
                    class_hash: tx.class_hash,
                    resource_bounds: tx.resource_bounds.clone(),
                    tip: tx.tip,
                    paymaster_data: tx.paymaster_data.clone(),
                    nonce_data_availability_mode: tx.nonce_data_availability_mode,
                    fee_data_availability_mode: tx.fee_data_availability_mode,
                    is_query: false,
                })
            }
        })),
    }
}

fn get_block_transaction_hash(transaction: &Transaction) -> FieldElement {
    match transaction {
        Transaction::Invoke(tx) => match tx {
            InvokeTransaction::V0(tx) => tx.transaction_hash,
            InvokeTransaction::V1(tx) => tx.transaction_hash,
            InvokeTransaction::V3(tx) => tx.transaction_hash,
        },
        Transaction::L1Handler(tx) => tx.transaction_hash,
        Transaction::Declare(tx) => *tx.transaction_hash(),
        Transaction::Deploy(tx) => tx.transaction_hash,
        Transaction::DeployAccount(tx) => match tx {
            DeployAccountTransaction::V1(tx) => tx.transaction_hash,
            DeployAccountTransaction::V3(tx) => tx.transaction_hash,
        },
    }
}

// // Try all transactions and count down until they all work
// async fn optimistic_repeat_simulate_until_success(
//     block_id: BlockId,
//     transactions: &[TransactionToSimulate],
// ) -> Vec<SimulatedTransaction> {
//     let broadcasted_transactions = transactions
//         .into_iter()
//         .map(|tx| tx.tx.clone())
//         .collect_vec();
//     for i in 0..transactions.len() {
//         let (juno_process, juno_rpc) = spawn_juno_checked().await;

//         let transactions_to_try = &broadcasted_transactions[0..transactions.len() - i];
//         println!("Trying {} tranactions", transactions_to_try.len());
//         let simulation_result = juno_rpc
//             .simulate_transactions(block_id, transactions_to_try, [])
//             .await;

//         if simulation_result.is_ok() {
//             kill_juno(juno_process);
//             return simulation_result.unwrap();
//         } else {
//             confirm_juno_killed(juno_process);
//         }
//     }
//     vec![]
// }

fn get_simulated_transaction_result(transaction: &SimulatedTransaction) -> TransactionResult {
    match &transaction.transaction_trace {
        TransactionTrace::Invoke(inv) => match &inv.execute_invocation {
            ExecuteInvocation::Success(_) => TransactionResult::Success,
            ExecuteInvocation::Reverted(tx) => TransactionResult::Revert {
                reason: tx.revert_reason.clone(),
            },
        },
        TransactionTrace::DeployAccount(_) => TransactionResult::DeployAccount,
        TransactionTrace::L1Handler(_) => TransactionResult::L1Handler,
        TransactionTrace::Declare(_) => TransactionResult::Declare,
    }
}

pub async fn simulate_main() -> Result<(), ManagerError> {
    let block_number = 610026;
    let mut juno_manager = JunoManager::new().await?;
    let block_report = juno_manager.simulate_block(block_number).await?;
    log_block_report(block_number, block_report);
    println!("//Done {block_number}");

    for block_number in 645000..645100 {
        let block_report = juno_manager.simulate_block(block_number).await?;
        log_block_report(block_number, block_report);
        println!("//Done {block_number}");
    }
    Ok(())
}
