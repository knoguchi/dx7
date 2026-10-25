//! BLE MIDI peripheral using BlueZ (Linux only).

use super::parse_ble_midi_packet;
use bluer::{
    adv::Advertisement,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicWrite, CharacteristicWriteMethod, Service,
    },
};
use crate::midi::CommandSink;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::oneshot;

/// BLE MIDI Service UUID (standard BLE MIDI spec).
const MIDI_SERVICE_UUID: bluer::Uuid =
    bluer::Uuid::from_u128(0x03B80E5A_EDE8_4B33_A751_6CE34EC4C700);

/// BLE MIDI I/O Characteristic UUID (standard BLE MIDI spec).
const MIDI_CHAR_UUID: bluer::Uuid =
    bluer::Uuid::from_u128(0x7772E5DB_3868_4112_A1A9_F2669D106BF3);

/// Handle to the running BLE MIDI server.
/// Dropping this stops advertising and shuts down the tokio runtime.
pub struct BleHandler {
    _runtime: std::thread::JoinHandle<()>,
    _shutdown_tx: oneshot::Sender<()>,
}

impl BleHandler {
    /// Start the BLE MIDI peripheral in a background thread.
    pub fn start(sink: Arc<CommandSink>) -> Result<Self, String> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::Builder::new()
            .name("ble-midi".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("Failed to create tokio runtime: {e}")));
                        return;
                    }
                };

                rt.block_on(async move {
                    match ble_server_task(sink, shutdown_rx, &ready_tx).await {
                        Ok(()) => {}
                        Err(e) => {
                            // If ready_tx hasn't been consumed yet, send the error
                            let _ = ready_tx.send(Err(e));
                        }
                    }
                });
            })
            .map_err(|e| format!("Failed to spawn BLE thread: {e}"))?;

        // Wait for the server to be ready (or fail)
        ready_rx
            .recv()
            .map_err(|_| "BLE thread exited unexpectedly".to_string())?
            .map_err(|e| e)?;

        Ok(BleHandler {
            _runtime: handle,
            _shutdown_tx: shutdown_tx,
        })
    }
}

/// Main async task: register GATT service, advertise, and handle connections.
async fn ble_server_task(
    sink: Arc<CommandSink>,
    shutdown_rx: oneshot::Receiver<()>,
    ready_tx: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let session = bluer::Session::new()
        .await
        .map_err(|e| format!("BlueZ session failed: {e}"))?;

    let adapter = session
        .default_adapter()
        .await
        .map_err(|e| format!("No Bluetooth adapter: {e}"))?;

    adapter
        .set_powered(true)
        .await
        .map_err(|e| format!("Failed to power on adapter: {e}"))?;

    // Create the MIDI I/O characteristic with Write (for receiving MIDI)
    // and Notify (required by BLE MIDI spec, though we don't send data).
    let (char_write_tx, mut char_write_rx) =
        tokio::io::duplex(256);

    let (_char_notify_tx, char_notify_rx) =
        tokio::sync::mpsc::channel::<Vec<u8>>(1);

    let app = Application {
        services: vec![Service {
            uuid: MIDI_SERVICE_UUID,
            primary: true,
            characteristics: vec![Characteristic {
                uuid: MIDI_CHAR_UUID,
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Io,
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let app_handle = adapter
        .serve_gatt_application(app)
        .await
        .map_err(|e| format!("Failed to register GATT application: {e}"))?;

    // Advertise as "DX7"
    let adv = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: vec![MIDI_SERVICE_UUID].into_iter().collect(),
        local_name: Some("DX7".to_string()),
        ..Default::default()
    };

    let adv_handle = adapter
        .advertise(adv)
        .await
        .map_err(|e| format!("Failed to start advertising: {e}"))?;

    // Signal that we're ready
    let _ = ready_tx.send(Ok(()));

    // Event loop: read BLE MIDI packets and forward to synth engine
    let mut buf = [0u8; 256];
    let mut shutdown_rx = shutdown_rx;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            result = tokio::io::AsyncReadExt::read(&mut char_write_rx, &mut buf) => {
                match result {
                    Ok(0) => {
                        // Client disconnected, keep listening for next connection
                        continue;
                    }
                    Ok(n) => {
                        sink.dispatch_all(parse_ble_midi_packet(&buf[..n]));
                    }
                    Err(_) => {
                        continue;
                    }
                }
            }
        }
    }

    // Clean up (handles dropped → advertising + GATT service stop)
    drop(adv_handle);
    drop(app_handle);

    Ok(())
}
