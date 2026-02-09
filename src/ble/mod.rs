mod bluez;
pub(crate) mod gatt;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};
use crate::config::AppConfig;
use crate::custom_command;
use crate::exec::{run_sh_checked, ExecOptions};
use crate::proto::ReassemblyBuffer;
use crate::{status, uuid, wifi};

use anyhow::Context;
use tracing::{info, warn};

pub async fn run(cfg: AppConfig) -> anyhow::Result<()> {
    let conn = zbus::Connection::system()
        .await
        .context("connect to system bus")?;

    let adapter_path = bluez::find_first_adapter(&conn).await?;
    info!(adapter = %adapter_path, "using bluetooth adapter");

    let service_uuid = uuid::bluez_uuid(uuid::SERVICE_ID)?;

    let custom = cfg.custom_config.clone();

    let app = gatt::GattApplication::new(&cfg, service_uuid.clone(), custom.clone())?;
    let g = app.handle();

    // WiFi configuration mutex — serializes concurrent BLE write requests (P0-2).
    let wifi_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // --- NOTIFY_MESSAGE: event-driven via watch channel (P0-4 + P1-5) ---
    let (notify_tx, _) = watch::channel::<String>(String::new());
    let notify_tx = Arc::new(notify_tx);

    if let Some(ch) = g.characteristic_by_uuid(uuid::NOTIFY_MESSAGE) {
        let ch_for_cb = ch.clone();
        let rx = notify_tx.subscribe();
        ch.on_start_notify(move || {
            let ch_ctl = ch_for_cb.clone();
            let ch = ch_for_cb.clone();
            let rx = rx.clone();
            ch_ctl.spawn_notify_task(async move {
                let mut rx = rx;
                while rx.changed().await.is_ok() {
                    let msg = rx.borrow_and_update().clone();
                    if ch.set_value(msg.as_bytes().to_vec()).await.is_err() {
                        break;
                    }
                }
            });
        });
    }

    // --- WIFI_NAME: poll every 5s with change detection (P1-6) ---
    if let Some(ch) = g.characteristic_by_uuid(uuid::WIFI_NAME) {
        let ch_for_cb = ch.clone();
        ch.on_start_notify(move || {
            let ch_ctl = ch_for_cb.clone();
            let ch = ch_for_cb.clone();
            ch_ctl.spawn_notify_task(async move {
                poll_notify_string_with_change_detection(
                    ch,
                    Duration::from_secs(5),
                    true,
                    || async { status::get_wifi_name_node_style().await },
                )
                .await;
            });
        });
    }

    // --- IP_ADDRESS: poll every 5s with change detection (P1-6) ---
    if let Some(ch) = g.characteristic_by_uuid(uuid::IP_ADDRESS) {
        let ch_for_cb = ch.clone();
        ch.on_start_notify(move || {
            let ch_ctl = ch_for_cb.clone();
            let ch = ch_for_cb.clone();
            ch_ctl.spawn_notify_task(async move {
                poll_notify_string_with_change_detection(
                    ch,
                    Duration::from_secs(5),
                    true,
                    || async { status::get_ip_address_node_style().await },
                )
                .await;
            });
        });
    }

    // --- INPUT (deprecated): parse via wifi::parse_wifi_request, serialized (P0-2 + P2-8) ---
    {
        let key = cfg.key.clone();
        let tx = notify_tx.clone();
        let wl = wifi_lock.clone();
        g.on_write_uuid(uuid::INPUT, move |value: Vec<u8>| {
            let key = key.clone();
            let tx = tx.clone();
            let wl = wl.clone();
            async move {
                let req = match wifi::parse_wifi_request(&value) {
                    Ok(r) => r,
                    Err(_) => {
                        let _ = tx.send("Wrong input syntax.".to_string());
                        return Ok(());
                    }
                };
                if req.key != key {
                    let _ = tx.send("Wrong input key.".to_string());
                    return Ok(());
                }

                let _guard = wl.lock().await;
                let msg = wifi::set_wifi_wpa_node_style(&req.ssid, &req.password).await;
                let _ = tx.send(msg);
                Ok(())
            }
        })?;
    }

    // --- INPUT_SEP: ReassemblyBuffer + parse + serialized WiFi (P0-1 + P0-2 + P2-8) ---
    {
        let key = cfg.key.clone();
        let tx = notify_tx.clone();
        let wl = wifi_lock.clone();
        let buf = Arc::new(Mutex::new(ReassemblyBuffer::with_default_max()));

        g.on_write_uuid(uuid::INPUT_SEP, move |value: Vec<u8>| {
            let key = key.clone();
            let tx = tx.clone();
            let wl = wl.clone();
            let buf = buf.clone();
            async move {
                let messages = {
                    let mut b = buf.lock().await;
                    match b.push(&value) {
                        Ok(msgs) => msgs,
                        Err(e) => {
                            warn!(error = ?e, "INPUT_SEP reassembly overflow");
                            return Ok(());
                        }
                    }
                };

                for payload in messages {
                    let req = match wifi::parse_wifi_request(&payload) {
                        Ok(r) => r,
                        Err(_) => {
                            let _ = tx.send("Invalid syntax.".to_string());
                            continue;
                        }
                    };
                    if req.key != key {
                        let _ = tx.send("Invalid key.".to_string());
                        continue;
                    }

                    let _guard = wl.lock().await;
                    if wifi::check_wlan0_managed_node_style().await {
                        wifi::set_wifi_nm_node_style(&req.ssid, &req.password).await;
                    } else {
                        let msg =
                            wifi::set_wifi_wpa_node_style(&req.ssid, &req.password).await;
                        let _ = tx.send(msg);
                    }
                }
                Ok(())
            }
        })?;
    }

    // --- CUSTOM_COMMAND_INPUT -> CUSTOM_COMMAND_NOTIFY ---
    // Event-driven via watch channel (P0-4 + P1-5), ReassemblyBuffer (P0-1),
    // reuse custom_command module (P2-8).
    if let Some(custom_cfg) = custom.as_ref() {
        let (cmd_tx, _) = watch::channel::<Arc<[u8]>>(Arc::from(Vec::new()));
        let cmd_tx = Arc::new(cmd_tx);

        let notify_ch = g
            .characteristic_by_uuid(uuid::CUSTOM_COMMAND_NOTIFY)
            .context("missing CUSTOM_COMMAND_NOTIFY characteristic")?;

        {
            let notify_ch_for_cb = notify_ch.clone();
            let rx = cmd_tx.subscribe();
            notify_ch.on_start_notify(move || {
                let ch_ctl = notify_ch_for_cb.clone();
                let ch = notify_ch_for_cb.clone();
                let rx = rx.clone();
                ch_ctl.spawn_notify_task(async move {
                    let mut rx = rx;
                    while rx.changed().await.is_ok() {
                        let chunk = rx.borrow_and_update().clone();
                        if ch.set_value(chunk.as_ref().to_vec()).await.is_err() {
                            break;
                        }
                    }
                });
            });
        }

        async fn response_chunked(tx: &Arc<watch::Sender<Arc<[u8]>>>, bytes: &[u8]) {
            let mut data = bytes.to_vec();
            data.extend_from_slice(crate::proto::END_TAG.as_bytes());

            for chunk in data.chunks(crate::proto::NOTIFY_CHUNK_SIZE) {
                let _ = tx.send(Arc::from(chunk));
                tokio::time::sleep(Duration::from_millis(crate::proto::NOTIFY_CHUNK_DELAY_MS))
                    .await;
            }
        }

        let key = cfg.key.clone();
        let commands = Arc::new(custom_cfg.commands.clone());
        let buf = Arc::new(Mutex::new(ReassemblyBuffer::with_default_max()));

        g.on_write_uuid(uuid::CUSTOM_COMMAND_INPUT, move |value: Vec<u8>| {
            let key = key.clone();
            let commands = commands.clone();
            let buf = buf.clone();
            let tx = cmd_tx.clone();
            async move {
                let messages = {
                    let mut b = buf.lock().await;
                    match b.push(&value) {
                        Ok(msgs) => msgs,
                        Err(e) => {
                            warn!(error = ?e, "CUSTOM_COMMAND_INPUT reassembly overflow");
                            return Ok(());
                        }
                    }
                };

                for payload in messages {
                    let req = match custom_command::parse_custom_command_request(&payload) {
                        Ok(r) => r,
                        Err(_) => {
                            response_chunked(&tx, b"Invalid syntax.").await;
                            continue;
                        }
                    };
                    if req.key != key {
                        response_chunked(&tx, b"key error").await;
                        continue;
                    }

                    let Some(command) =
                        custom_command::command_for_suffix(&commands, &req.suffix)
                    else {
                        response_chunked(&tx, b"Command not found.").await;
                        continue;
                    };
                    let command = command.to_string();

                    response_chunked(&tx, b"exec done.\n").await;

                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let opts = ExecOptions::default();
                        match run_sh_checked(&command, opts).await {
                            Ok(out) => {
                                response_chunked(&tx2, &out.stdout).await;
                            }
                            Err(e) => {
                                let msg = format!("exec error: {e}");
                                response_chunked(&tx2, msg.as_bytes()).await;
                            }
                        }
                    });
                }

                Ok(())
            }
        })?;
    }

    // --- CUSTOM_INFO value characteristics ---
    if let Some(custom_cfg) = custom.as_ref() {
        for (i, item) in custom_cfg.info.iter().enumerate() {
            let uuid_end = format!("{:04x}", i + 1);
            let value_uuid = format!("{}{}", uuid::CUSTOM_INFO_PREFIX, uuid_end);
            let Some(ch) = g.characteristic_by_uuid(&value_uuid) else {
                continue;
            };

            let command = item.command.clone();
            let interval_s = item.interval.unwrap_or(10);
            let ch_for_cb = ch.clone();
            ch.on_start_notify(move || {
                let ch_ctl = ch_for_cb.clone();
                let ch = ch_for_cb.clone();
                let command = command.clone();
                ch_ctl.spawn_notify_task(async move {
                    let opts = ExecOptions::default();

                    let mut last = match run_sh_checked(&command, opts.clone()).await {
                        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
                        Err(_) => "cmd error".to_string(),
                    };
                    if ch.set_value(last.as_bytes().to_vec()).await.is_err() {
                        return;
                    }

                    loop {
                        tokio::time::sleep(Duration::from_secs(interval_s)).await;
                        let v2 = match run_sh_checked(&command, opts.clone()).await {
                            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
                            Err(_) => "cmd error".to_string(),
                        };
                        if v2 == last {
                            continue;
                        }
                        last = v2.clone();
                        if ch.set_value(v2.as_bytes().to_vec()).await.is_err() {
                            break;
                        }
                    }
                });
            });
        }
    }

    // Export D-Bus objects first (BlueZ will immediately call GetManagedObjects).
    app.export(&conn).await?;

    // Register GATT application + advertisement.
    let bluez = bluez::BluezClient::new(&conn, &adapter_path).await?;
    bluez.register_gatt_application(app.root_path()).await?;
    bluez.register_advertisement(app.advertisement_path()).await?;

    info!(service_uuid = %service_uuid, "ble gatt server registered");
    info!("waiting (Ctrl+C to stop)");

    // Keep running until interrupted.
    tokio::signal::ctrl_c().await.context("wait for Ctrl+C")?;
    info!("stopping");

    // Best-effort cleanup.
    if let Err(e) = bluez.unregister_advertisement(app.advertisement_path()).await {
        warn!(error = %e, "failed to unregister advertisement");
    }
    if let Err(e) = bluez.unregister_gatt_application(app.root_path()).await {
        warn!(error = %e, "failed to unregister gatt application");
    }

    Ok(())
}

async fn poll_notify_string_with_change_detection<F, Fut>(
    ch: gatt::CharacteristicHandle,
    interval: Duration,
    send_immediately: bool,
    mut fetch: F,
) where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = String> + Send,
{
    let mut last = String::new();

    if send_immediately {
        let v = fetch().await;
        last = v.clone();
        if ch.set_value(v.as_bytes().to_vec()).await.is_err() {
            return;
        }
    }

    loop {
        tokio::time::sleep(interval).await;
        let v = fetch().await;
        if v == last {
            continue;
        }
        last = v.clone();
        if ch.set_value(v.as_bytes().to_vec()).await.is_err() {
            break;
        }
    }
}
