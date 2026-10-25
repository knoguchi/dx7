//! BLE MIDI peripheral using BlueZ (Linux only).

use super::parse_ble_midi_packet;
use crate::midi::CommandSink;
use bluer::{
    adv::Advertisement,
    gatt::{
        local::{
            characteristic_control, Application, Characteristic, CharacteristicControlEvent,
            CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead,
            CharacteristicWrite, CharacteristicWriteMethod, Service,
        },
        CharacteristicReader, CharacteristicWriter,
    },
};
use futures::{FutureExt, StreamExt};
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

    // The MIDI I/O characteristic. BLE-MIDI requires Read (returns no
    // data), Write Without Response (how centrals send us MIDI) and
    // Notify (how we would send MIDI; unused). With the Io write method
    // BlueZ hands us a socket per connecting central via the control
    // stream, which is the only way the written bytes reach us.
    let (char_control, char_control_handle) = characteristic_control();

    let app = Application {
        services: vec![Service {
            uuid: MIDI_SERVICE_UUID,
            primary: true,
            characteristics: vec![Characteristic {
                uuid: MIDI_CHAR_UUID,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|_req| async { Ok(Vec::new()) }.boxed()),
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write: true,
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Io,
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),
                control_handle: char_control_handle,
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

    // Event loop: accept write sessions from the control stream, then read
    // BLE MIDI packets (one datagram per write) and forward them to the synth.
    let mut char_control = char_control;
    let mut shutdown_rx = shutdown_rx;
    let mut reader: Option<CharacteristicReader> = None;
    // Kept alive so the notify session stays open; we never send.
    let mut _notify_writer: Option<CharacteristicWriter> = None;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            evt = char_control.next() => {
                match evt {
                    Some(CharacteristicControlEvent::Write(req)) => {
                        let addr = req.device_address();
                        let mtu = req.mtu();
                        match req.accept() {
                            Ok(r) => {
                                eprintln!("BLE MIDI: {addr} connected (MTU {mtu})");
                                reader = Some(r);
                            }
                            Err(e) => eprintln!("BLE MIDI: failed to accept write from {addr}: {e}"),
                        }
                    }
                    Some(CharacteristicControlEvent::Notify(w)) => {
                        eprintln!("BLE MIDI: {} subscribed to notifications", w.device_address());
                        _notify_writer = Some(w);
                    }
                    None => {
                        eprintln!("BLE MIDI: GATT application unregistered");
                        break;
                    }
                }
            }
            packet = async {
                match &reader {
                    Some(r) => r.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match packet {
                    Ok(bytes) if bytes.is_empty() => {
                        eprintln!("BLE MIDI: central disconnected");
                        reader = None;
                    }
                    Ok(bytes) => {
                        sink.dispatch_all(parse_ble_midi_packet(&bytes));
                    }
                    Err(e) => {
                        eprintln!("BLE MIDI: read error ({e}); waiting for reconnect");
                        reader = None;
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
