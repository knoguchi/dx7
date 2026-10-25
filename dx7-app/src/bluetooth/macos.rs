//! BLE MIDI peripheral using CoreBluetooth (macOS).

use super::parse_ble_midi_packet;
use crate::midi::CommandSink;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_bluetooth::*;
use objc2_foundation::*;
use std::sync::{Arc, Mutex};

/// BLE MIDI Service UUID (standard BLE MIDI spec).
const MIDI_SERVICE_UUID_STR: &str = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700";

/// BLE MIDI I/O Characteristic UUID (standard BLE MIDI spec).
const MIDI_CHAR_UUID_STR: &str = "7772E5DB-3868-4112-A1A9-F2669D106BF3";

struct DelegateIvars {
    sink: Arc<CommandSink>,
    ready_tx: Mutex<Option<std::sync::mpsc::Sender<Result<(), String>>>>,
}

/// Send a ready/error signal through the oneshot-style channel in the ivars.
fn signal_ready(delegate: &MidiPeripheralDelegate, result: Result<(), String>) {
    if let Some(tx) = delegate.ivars().ready_tx.lock().unwrap().take() {
        let _ = tx.send(result);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    struct MidiPeripheralDelegate;

    unsafe impl NSObjectProtocol for MidiPeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for MidiPeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn peripheral_manager_did_update_state(&self, peripheral: &CBPeripheralManager) {
            let state = unsafe { peripheral.state() };
            if state == CBManagerState::PoweredOn {
                setup_gatt(peripheral);
            } else {
                signal_ready(self, Err(format!("Bluetooth not available (state {:?})", state)));
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn peripheral_manager_did_add_service(
            &self,
            peripheral: &CBPeripheralManager,
            _service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(err) = error {
                eprintln!("BLE MIDI: Failed to add service: {err}");
                signal_ready(self, Err(format!("Failed to add GATT service: {err}")));
                return;
            }

            // Service added successfully — start advertising
            unsafe {
                let service_uuid =
                    CBUUID::UUIDWithString(&NSString::from_str(MIDI_SERVICE_UUID_STR));
                let uuids = NSArray::from_retained_slice(&[service_uuid]);
                let name = NSString::from_str("DX7");

                let keys: &[&NSString] = &[
                    CBAdvertisementDataLocalNameKey,
                    CBAdvertisementDataServiceUUIDsKey,
                ];
                let objects: Vec<Retained<AnyObject>> = vec![
                    Retained::into_super(name).into(),
                    Retained::into_super(uuids).into(),
                ];
                let dict = NSDictionary::from_retained_objects(keys, &objects);
                peripheral.startAdvertising(Some(&dict));
            }
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn peripheral_manager_did_start_advertising(
            &self,
            _peripheral: &CBPeripheralManager,
            error: Option<&NSError>,
        ) {
            if let Some(err) = error {
                signal_ready(self, Err(format!("Advertising failed: {err}")));
            } else {
                signal_ready(self, Ok(()));
            }
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn peripheral_manager_central_did_subscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            _central: &CBCentral,
            characteristic: &CBCharacteristic,
        ) {
            let uuid = unsafe { characteristic.UUID() };
            eprintln!("BLE MIDI: central subscribed to {uuid:?}");
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn peripheral_manager_central_did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            _central: &CBCentral,
            characteristic: &CBCharacteristic,
        ) {
            let uuid = unsafe { characteristic.UUID() };
            eprintln!("BLE MIDI: central unsubscribed from {uuid:?}");
        }

        #[unsafe(method(peripheralManager:didReceiveReadRequest:))]
        fn peripheral_manager_did_receive_read_request(
            &self,
            peripheral: &CBPeripheralManager,
            request: &CBATTRequest,
        ) {
            eprintln!("BLE MIDI: received read request");
            unsafe {
                // Return empty data for the MIDI characteristic
                request.setValue(Some(&NSData::new()));
                peripheral.respondToRequest_withResult(request, CBATTError::Success);
            }
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn peripheral_manager_did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            let mut responded = false;
            for request in requests {
                if let Some(data) = unsafe { request.value() } {
                    let bytes = data.to_vec();
                    eprintln!("BLE MIDI: recv {} bytes: {:02X?}", bytes.len(), bytes);
                    self.ivars().sink.dispatch_all(parse_ble_midi_packet(&bytes));
                }
                // Respond to the first request to acknowledge all writes
                if !responded {
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success);
                    }
                    responded = true;
                }
            }
        }
    }
);

impl MidiPeripheralDelegate {
    fn new(ivars: DelegateIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

/// Set up the BLE MIDI GATT service and characteristic on the peripheral.
fn setup_gatt(peripheral: &CBPeripheralManager) {
    unsafe {
        let service_uuid = CBUUID::UUIDWithString(&NSString::from_str(MIDI_SERVICE_UUID_STR));
        let char_uuid = CBUUID::UUIDWithString(&NSString::from_str(MIDI_CHAR_UUID_STR));

        let properties = CBCharacteristicProperties::Read
            | CBCharacteristicProperties::WriteWithoutResponse
            | CBCharacteristicProperties::Notify;
        let permissions = CBAttributePermissions::Readable | CBAttributePermissions::Writeable;

        let characteristic = CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &char_uuid,
            properties,
            None, // dynamic value
            permissions,
        );

        let service = CBMutableService::initWithType_primary(
            CBMutableService::alloc(),
            &service_uuid,
            true,
        );

        // CBMutableCharacteristic → CBCharacteristic (one into_super)
        let char_base: Retained<CBCharacteristic> = Retained::into_super(characteristic);
        let chars = NSArray::from_retained_slice(&[char_base]);
        service.setCharacteristics(Some(&chars));

        peripheral.addService(&service);
    }
}

/// Handle to the running BLE MIDI peripheral.
/// Dropping this stops advertising and shuts down the background thread.
pub struct BleHandler {
    _thread: std::thread::JoinHandle<()>,
    _shutdown_tx: std::sync::mpsc::Sender<()>,
}

impl BleHandler {
    /// Start the BLE MIDI peripheral in a background thread.
    pub fn start(sink: Arc<CommandSink>) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::Builder::new()
            .name("ble-midi".into())
            .spawn(move || {
                let delegate = MidiPeripheralDelegate::new(DelegateIvars {
                    sink,
                    ready_tx: Mutex::new(Some(ready_tx)),
                });

                // Create a serial dispatch queue for CoreBluetooth callbacks.
                // None = serial queue (the default).
                let queue = dispatch2::DispatchQueue::new("com.dx7.ble-midi", None);

                // Create the peripheral manager — delegate callbacks fire on `queue`.
                let _manager = unsafe {
                    CBPeripheralManager::initWithDelegate_queue(
                        CBPeripheralManager::alloc(),
                        Some(ProtocolObject::from_ref(&*delegate)),
                        Some(&queue),
                    )
                };

                // Block until shutdown signal (sender dropped or explicit send).
                let _ = shutdown_rx.recv();
                // _manager, delegate, and queue are dropped here, stopping BLE.
            })
            .map_err(|e| format!("Failed to spawn BLE thread: {e}"))?;

        // Wait for the peripheral to finish setup (or report an error).
        ready_rx
            .recv()
            .map_err(|_| "BLE thread exited unexpectedly".to_string())?
            .map_err(|e| e)?;

        Ok(BleHandler {
            _thread: handle,
            _shutdown_tx: shutdown_tx,
        })
    }
}
