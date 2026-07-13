//! Independent terminal-state verification for all live mode surfaces.
//!
//! Wallet-local records remain scenario observations. Only the checks in this
//! module may emit top-level chain verification rows.

use super::*;

pub(super) async fn verify_mode1_transactions(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    base_node_url: &str,
    tx_ids: &[String],
    scenario: ScenarioName,
    required_depth: u64,
) -> anyhow::Result<Mode1VerificationResult> {
    let ids = tx_ids
        .iter()
        .filter_map(|tx_id| tx_id.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(Mode1VerificationResult::default());
    }
    let response = client
        .get_transaction_info(grpc::GetTransactionInfoRequest {
            transaction_ids: ids,
        })
        .await?
        .into_inner();
    let tip_height = base_node_tip_height(base_node_url).await.ok();
    let mut result = Mode1VerificationResult::default();
    for info in response.transactions {
        let status_value = u32::try_from(info.status).unwrap_or_default();
        let mined_height = (info.mined_in_block_height > 0).then_some(info.mined_in_block_height);
        let confirmations = mined_height
            .zip(tip_height)
            .map(|(mined, tip)| tip.saturating_sub(mined));
        let independently_mined = if terminal_ok_status(status_value) {
            mode1_outputs_exist_at_height(
                base_node_url,
                info.mined_in_block_height,
                &info.output_commitments,
            )
            .await
            .unwrap_or(false)
        } else {
            false
        };
        let confirmed = terminal_ok_status(status_value)
            && independently_mined
            && confirmations.is_some_and(|depth| depth >= required_depth);
        let tx_id = info.tx_id.to_string();
        result.shapes.insert(
            tx_id.clone(),
            TransactionShape {
                input_count: u32::try_from(info.input_commitments.len()).unwrap_or(u32::MAX),
                total_output_count: u32::try_from(info.output_commitments.len())
                    .unwrap_or(u32::MAX),
                output_commitments: info.output_commitments.iter().map(hex::encode).collect(),
            },
        );
        result.transactions.push(VerifiedTransaction {
            tx_id,
            status_value,
            mode: "old_wallet".to_string(),
            scenario: scenario.as_str().to_string(),
            amount_microtari: Some(info.amount),
            fee_microtari: Some(info.fee),
            mined_height,
            confirmations,
            min_confirmations: Some(required_depth),
            tip_height,
            confirmed,
        });
    }
    Ok(result)
}

pub(super) async fn wait_for_mode1_summary_verification(
    client: &mut WalletGrpcClient<tonic::transport::Channel>,
    base_node_url: &str,
    summary: &mut Mode1TransferSummary,
    scenario: ScenarioName,
    verification_start_offset_ms: u128,
    timeout: Duration,
    required_depth: u64,
) {
    if summary.tx_ids.is_empty() {
        return;
    }
    let start = Instant::now();
    let mut interval = time::interval(Duration::from_secs(10));
    let mut latest = Vec::new();
    let mut confirmed_at = BTreeMap::new();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        let call_timeout = remaining.min(Duration::from_secs(30));
        match time::timeout(
            call_timeout,
            verify_mode1_transactions(
                client,
                base_node_url,
                &summary.tx_ids,
                scenario,
                required_depth,
            ),
        )
        .await
        {
            Ok(Ok(verification)) => {
                let verified = verification.transactions;
                summary.transaction_shapes.extend(verification.shapes);
                let observed_at =
                    verification_start_offset_ms.saturating_add(start.elapsed().as_millis());
                for tx in verified.iter().filter(|tx| tx.confirmed) {
                    confirmed_at.entry(tx.tx_id.clone()).or_insert(observed_at);
                }
                let all_terminal = !verified.is_empty()
                    && verified.len() >= summary.tx_ids.len()
                    && verified.iter().all(|tx| tx.confirmed);
                println!(
                    "mode1 verification wait: scenario={} confirmed={}/{} statuses={}",
                    scenario.as_str(),
                    verified.iter().filter(|tx| tx.confirmed).count(),
                    summary.tx_ids.len(),
                    verified
                        .iter()
                        .map(|tx| format!("{}:{}", tx.tx_id, tx.status_value))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                latest = verified;
                if all_terminal {
                    break;
                }
            }
            Ok(Err(error)) => {
                summary
                    .errors
                    .push(format!("mode1 chain verification failed: {error:#}"));
                break;
            }
            Err(_) => {
                summary.errors.push(format!(
                    "mode1 chain verification timed out after {}s for {} tx ids",
                    call_timeout.as_secs(),
                    summary.tx_ids.len()
                ));
                break;
            }
        }
        interval.tick().await;
    }
    for timing in &mut summary.tx_timings {
        let confirmed_at_ms = timing
            .get("tx_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|tx_id| confirmed_at.get(tx_id).copied());
        let submit_offset_ms = timing_u128(timing, "submit_offset_ms").unwrap_or_default();
        if let Some(confirmed_at_ms) = confirmed_at_ms
            && let Some(map) = timing.as_object_mut()
        {
            map.insert(
                "broadcast_to_confirmed_at_c_min_ms".to_string(),
                serde_json::json!(confirmed_at_ms.saturating_sub(submit_offset_ms)),
            );
        }
    }
    summary.tx_infos.extend(latest);
    summary.backfill_verified_fee_total();
}

async fn mode1_outputs_exist_at_height(
    base_node_url: &str,
    mined_height: u64,
    expected_commitments: &[Vec<u8>],
) -> anyhow::Result<bool> {
    if mined_height == 0 || expected_commitments.is_empty() {
        return Ok(false);
    }
    let client = base_node_http_client()?;
    let mut header_url = url::Url::parse(base_node_url)?.join("/get_header_by_height")?;
    header_url
        .query_pairs_mut()
        .append_pair("height", &mined_height.to_string());
    let header: serde_json::Value = client
        .get(header_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let header_hash = header["hash"]
        .as_array()
        .context("Mode 1 header proof omitted hash")?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .context("Mode 1 header hash contains a non-byte value")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut outputs_url = url::Url::parse(base_node_url)?.join("/get_utxos_by_block")?;
    outputs_url
        .query_pairs_mut()
        .append_pair("header_hash", &hex::encode(header_hash));
    let block: tari_transaction_components::rpc::models::GetUtxosByBlockResponse = client
        .get(outputs_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(expected_commitments.iter().all(|expected| {
        block
            .outputs
            .iter()
            .any(|output| output.commitment().as_bytes() == expected)
    }))
}

pub(super) async fn verify_mode2_transactions_with_client(
    config: &Config,
    db_path: &Path,
    tx_ids: &[String],
    scenario: ScenarioName,
    client: &reqwest::Client,
) -> anyhow::Result<Mode2VerificationResult> {
    if tx_ids.is_empty() || !db_path.exists() {
        return Ok(Mode2VerificationResult::default());
    }
    let conn = Connection::open(db_path)?;
    let tip_height = base_node_tip_height_with_client(client, &config.network.base_node_http_url)
        .await
        .ok();
    let mut result = Mode2VerificationResult::default();
    for tx_id in tx_ids {
        let Ok(parsed) = tx_id.parse::<u64>() else {
            continue;
        };
        let row = mode2_completed_transaction_row(&conn, parsed as i64)?;
        if let Some(row) = row.as_ref()
            && let Ok(shape) = mode2_transaction_shape(&row.serialized_transaction)
        {
            result.transaction_shapes.insert(tx_id.clone(), shape);
        }
        let (status_value, confirmed, mined_height, source, query_observation) = match row.as_ref()
        {
            Some(row) => {
                let kernel_query =
                    mode2_kernel_query_from_serialized_transaction(&row.serialized_transaction);
                match kernel_query {
                    Ok(kernel_query) => {
                        let query = query_mode2_transaction(
                            client,
                            &config.network.base_node_http_url,
                            &kernel_query,
                        )
                        .await;
                        match query {
                            Ok(response) => {
                                let (status_value, confirmed) = mode2_transaction_query_status(
                                    &response,
                                    tip_height,
                                    config.benchmark.c_min,
                                );
                                let mined_height = response.mined_height.or(row
                                    .confirmation_height
                                    .or(row.mined_height)
                                    .and_then(|height| u64::try_from(height).ok()));
                                (
                                    status_value,
                                    confirmed,
                                    mined_height,
                                    "base_node_transaction_query",
                                    mode2_query_observation(
                                        tx_id,
                                        row,
                                        Some(&kernel_query),
                                        Some(&response),
                                        tip_height,
                                        confirmed,
                                        None,
                                    ),
                                )
                            }
                            Err(error) => {
                                let (status_value, db_confirmed) =
                                    mode2_completed_transaction_status(&row.status);
                                (
                                    status_value,
                                    false,
                                    row.confirmation_height
                                        .or(row.mined_height)
                                        .and_then(|height| u64::try_from(height).ok()),
                                    "wallet_db_observed",
                                    mode2_query_observation(
                                        tx_id,
                                        row,
                                        Some(&kernel_query),
                                        None,
                                        tip_height,
                                        db_confirmed,
                                        Some(format!("{error:#}")),
                                    ),
                                )
                            }
                        }
                    }
                    Err(error) => {
                        let (status_value, db_confirmed) =
                            mode2_completed_transaction_status(&row.status);
                        (
                            status_value,
                            false,
                            row.confirmation_height
                                .or(row.mined_height)
                                .and_then(|height| u64::try_from(height).ok()),
                            "wallet_db_observed",
                            mode2_query_observation(
                                tx_id,
                                row,
                                None,
                                None,
                                tip_height,
                                db_confirmed,
                                Some(format!("{error:#}")),
                            ),
                        )
                    }
                }
            }
            None => (
                0,
                false,
                None,
                "wallet_db_observed",
                serde_json::json!({
                    "tx_id": tx_id,
                    "verification_source": "wallet_db_observed",
                    "wallet_db_status": "not_found",
                    "confirmed": false
                }),
            ),
        };

        let confirmations = mined_height
            .zip(tip_height)
            .map(|(mined, tip)| tip.saturating_sub(mined));
        result.observed_transactions.push(VerifiedTransaction {
            tx_id: tx_id.clone(),
            status_value,
            mode: "new_wallet".to_string(),
            scenario: scenario.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: row
                .as_ref()
                .and_then(|row| {
                    mode2_kernel_query_from_serialized_transaction(&row.serialized_transaction).ok()
                })
                .and_then(|query| query.fee_microtari),
            mined_height,
            confirmations,
            min_confirmations: Some(config.benchmark.c_min),
            tip_height,
            confirmed,
        });
        result.observations.push(query_observation);
        if source == "base_node_transaction_query" {
            result.used_base_node_query = true;
        }
    }
    Ok(result)
}

pub(super) fn mode2_completed_transaction_row(
    conn: &Connection,
    tx_id: i64,
) -> anyhow::Result<Option<Mode2CompletedTransactionRow>> {
    let row = conn.query_row(
        r#"
        SELECT pending_tx_id, status, mined_height, confirmation_height, sent_payref, serialized_transaction
        FROM completed_transactions
        WHERE id = ?1
        "#,
        [tx_id],
        |row| {
            Ok(Mode2CompletedTransactionRow {
                pending_tx_id: row.get::<_, String>(0)?,
                status: row.get::<_, String>(1)?,
                mined_height: row.get::<_, Option<i64>>(2)?,
                confirmation_height: row.get::<_, Option<i64>>(3)?,
                sent_payref: row.get::<_, Option<String>>(4)?,
                serialized_transaction: row.get::<_, Vec<u8>>(5)?,
            })
        },
    );
    match row {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn mode2_kernel_query_from_serialized_transaction(
    serialized_transaction: &[u8],
) -> anyhow::Result<Mode2KernelQuery> {
    let transaction: Transaction = serde_json::from_slice(serialized_transaction)
        .context("deserializing Mode 2 transaction")?;
    let kernel = transaction
        .body()
        .kernels()
        .first()
        .context("Mode 2 transaction has no kernel")?;
    Ok(Mode2KernelQuery {
        excess_sig_nonce: kernel
            .excess_sig
            .get_compressed_public_nonce()
            .as_bytes()
            .to_vec(),
        excess_sig: kernel.excess_sig.get_signature().as_bytes().to_vec(),
        fee_microtari: Some(kernel.fee.0),
    })
}

fn mode2_transaction_shape(serialized_transaction: &[u8]) -> anyhow::Result<TransactionShape> {
    let transaction: Transaction = serde_json::from_slice(serialized_transaction)
        .context("deserializing Mode 2 transaction shape")?;
    Ok(TransactionShape {
        input_count: u32::try_from(transaction.body().inputs().len())
            .context("Mode 2 transaction input count exceeds u32")?,
        total_output_count: u32::try_from(transaction.body().outputs().len())
            .context("Mode 2 transaction output count exceeds u32")?,
        output_commitments: transaction
            .body()
            .outputs()
            .iter()
            .map(|output| hex::encode(output.commitment().as_bytes()))
            .collect(),
    })
}

pub(super) async fn query_mode2_transaction(
    client: &reqwest::Client,
    base_node_url: &str,
    query: &Mode2KernelQuery,
) -> anyhow::Result<TxQueryResponse> {
    let url = mode2_transaction_query_url(base_node_url, query)?;
    let response = client
        .get(url)
        .send()
        .await
        .context("requesting base-node transaction query")?
        .error_for_status()
        .context("base-node transaction query HTTP status")?
        .json::<TxQueryResponse>()
        .await
        .context("decoding base-node transaction query")?;
    Ok(response)
}

pub(super) fn mode2_transaction_query_url(
    base_node_url: &str,
    query: &Mode2KernelQuery,
) -> anyhow::Result<url::Url> {
    let mut url = base_node_endpoint_url(base_node_url, "/transactions")?;
    url.query_pairs_mut()
        .append_pair("excess_sig_nonce", &hex::encode(&query.excess_sig_nonce))
        .append_pair("excess_sig_sig", &hex::encode(&query.excess_sig));
    Ok(url)
}

pub(super) fn mode2_transaction_query_status(
    response: &TxQueryResponse,
    tip_height: Option<u64>,
    required_depth: u64,
) -> (u32, bool) {
    match response.location {
        TxLocation::Mined => {
            let confirmed = response
                .mined_height
                .zip(tip_height)
                .is_some_and(|(mined, tip)| tip >= mined.saturating_add(required_depth));
            if confirmed {
                (TX_MINED_CONFIRMED_STATUS, true)
            } else {
                (2, false)
            }
        }
        TxLocation::InMempool => (1, false),
        TxLocation::NotStored | TxLocation::None => (0, false),
    }
}

fn mode2_query_observation(
    tx_id: &str,
    row: &Mode2CompletedTransactionRow,
    kernel_query: Option<&Mode2KernelQuery>,
    response: Option<&TxQueryResponse>,
    tip_height: Option<u64>,
    confirmed: bool,
    query_error: Option<String>,
) -> serde_json::Value {
    let (db_status_value, db_confirmed) = mode2_completed_transaction_status(&row.status);
    serde_json::json!({
        "tx_id": tx_id,
        "pending_tx_id": row.pending_tx_id,
        "sent_payref": row.sent_payref,
        "verification_source": if response.is_some() { "base_node_transaction_query" } else { "wallet_db_observed" },
        "wallet_db_status": row.status,
        "wallet_db_status_value": db_status_value,
        "wallet_db_confirmed": db_confirmed,
        "wallet_db_mined_height": row.mined_height,
        "wallet_db_confirmation_height": row.confirmation_height,
        "base_node_query_location": response.map(|response| format!("{:?}", response.location)),
        "base_node_query_mined_height": response.and_then(|response| response.mined_height),
        "base_node_tip_height": tip_height,
        "fee_microtari": kernel_query.and_then(|query| query.fee_microtari),
        "confirmed": confirmed,
        "query_error": query_error
    })
}
