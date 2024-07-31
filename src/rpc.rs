use std::result::Result;
use std::sync::{Arc, Mutex};

use esp32_nimble::{BLEAdvertisedDevice, BLEClient, BLEDevice, BLERemoteService, uuid128};
use esp32_nimble::utilities::BleUuid;
use lazy_static::lazy_static;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use serde_json::Map;
use tokio::sync::mpsc::Sender;

use crate::AppState;

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub params: Map<String, serde_json::Value>,
}
const PACKET_TYPE_RESPONSE: &str = "response";
const PACKET_TYPE_EVENT: &str = "event";
#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResponse {
    pub id: u64,
    pub packet_type: String,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

impl Default for RpcResponse {
    fn default() -> Self {
        Self {
            id: 0,
            packet_type: PACKET_TYPE_RESPONSE.to_string(),
            error: None,
            result: None,
        }
    }
    
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Event<T> {
    pub name: String,
    pub packet_type: String,
    pub data: Option<T>,
}

impl Default for Event<Value> {
    fn default() -> Self {
        Self {
            name: "".to_string(),
            packet_type: PACKET_TYPE_EVENT.to_string(),
            data: None,
        }
    }
}

impl Default for Event<String> {
    fn default() -> Self {
        Self {
            name: "".to_string(),
            packet_type: PACKET_TYPE_EVENT.to_string(),
            data: None,
        }
    }
}


#[derive(Serialize, Deserialize, Debug)]
pub struct MyBLEServiceData {
    pub uuid: String,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MyBLEAdvertisedDevice {
    pub name: String,
    pub address: String,
    pub rssi: i32,
    pub adv_type: String,
    pub adv_flags: String,
    pub raw_data: Vec<u8>,
    pub service_uuids: Vec<String>,
    pub service_data_list: Vec<MyBLEServiceData>,
    pub manufacture_data: Option<Vec<u8>>,

}

impl From<BLEAdvertisedDevice> for MyBLEAdvertisedDevice {
    fn from(device: BLEAdvertisedDevice) -> Self {
        let adv_flags: Option<esp32_nimble::enums::AdvFlag> = device.adv_flags();
        let adv_flags_str = match adv_flags {
            Some(flag) => std::format!("{:?}", flag),
            None => "".to_string(),
        };
        Self {
            name: device.name().to_string(),
            address: device.addr().to_string(),
            rssi: device.rssi(),
            adv_type: std::format!("{:?}", device.adv_type()),
            adv_flags: adv_flags_str,
            raw_data: device.raw_data.clone(),
            service_uuids: device.get_service_uuids().map(|uuid| uuid.to_string()).collect(),
            service_data_list: device.get_service_data_list().map(|data| {
                MyBLEServiceData {
                    uuid: data.uuid().to_string(),
                    data: data.data().to_vec(),
                }
            }).collect(),
            manufacture_data: device.get_manufacture_data().map(|data| data.to_vec()),

        }
    }
}

async fn add(params: &Map<String, Value>) -> Result<Value, String> {
    let a = match params.get("a").and_then(|v| v.as_i64()) {
        Some(num) => num,
        None => return Err("Parameter 'a' is missing or not an integer".to_string()),
    };
    let b = match params.get("b").and_then(|v| v.as_i64()) {
        Some(num) => num,
        None => return Err("Parameter 'b' is missing or not an integer".to_string()),
    };
    Ok(Value::Number(Number::from(a + b)))
}

async fn sub(params: &Map<String, Value>) -> Result<Value, String> {
    let a = match params.get("a").and_then(|v| v.as_i64()) {
        Some(num) => num,
        None => return Err("Parameter 'a' is missing or not an integer".to_string()),
    };
    let b = match params.get("b").and_then(|v| v.as_i64()) {
        Some(num) => num,
        None => return Err("Parameter 'b' is missing or not an integer".to_string()),
    };
    Ok(Value::Number(Number::from(a - b)))
}

async fn get_uuid() -> Result<Value, String> {
    let uuid = crate::UUID.lock().unwrap().clone();
    Ok(Value::String(uuid.to_string()))
}

async fn get_info() -> Result<Value, String> {
    let mut map = Map::new();
    let heap_caps_get_free_size = unsafe { esp_idf_sys::heap_caps_get_free_size(0) };
    map.insert(
        "heap_caps_get_free_size".to_string(),
        Value::Number(Number::from(heap_caps_get_free_size)),
    );
    let esp_get_minimum_free_heap_size = unsafe { esp_idf_sys::esp_get_minimum_free_heap_size() };
    map.insert(
        "esp_get_minimum_free_heap_size".to_string(),
        Value::Number(Number::from(esp_get_minimum_free_heap_size)),
    );
    let esp_get_free_heap_size = unsafe { esp_idf_sys::esp_get_free_heap_size() };
    map.insert(
        "esp_get_free_heap_size".to_string(),
        Value::Number(Number::from(esp_get_free_heap_size)),
    );

    Ok(Value::Object(map))
}

async fn on_ble_scan_result(device: BLEAdvertisedDevice) -> Result<String, String> {
    let event = Event {
        name: "ble_scan_result".to_string(),
        packet_type: PACKET_TYPE_EVENT.to_string(),
        data: Some(MyBLEAdvertisedDevice::from(device)),
    };
    let event_str_res = serde_json::to_string(&event);
    if let Err(e) = event_str_res {
        return Err(format!("failed to serialize event: {:?}", e));
    }
    let event_str = event_str_res.unwrap();
    return Ok(event_str);
}

async fn bluetooth_start_scan(tx: Sender<Vec<u8>>) -> Result<Value, String> {
    let ble_device = BLEDevice::take();
    let ble_scan = ble_device.get_scan();

    let devices_queue: Arc<Mutex<Vec<BLEAdvertisedDevice>>> = Arc::new(Mutex::new(Vec::new()));
    let devices_queue2 = devices_queue.clone();
    ble_scan
        .active_scan(true)
        .interval(100)
        .window(99)
        .on_result(move |_scan, device| {
            let devices_queue = devices_queue.clone();
            let mut devices_queue = devices_queue.lock().unwrap();
            devices_queue.push(device.clone());
        });



    let tx_clone = tx.clone();
    let listener = tokio::spawn(async move {
        let devices_queue = Arc::clone(&devices_queue2);
        
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            if devices_queue.lock().unwrap().is_empty() {
                continue;
            }
            let device = devices_queue.lock().unwrap().remove(0);
            info!("Sending scan result for {:?}", device.addr());
            let res = on_ble_scan_result(device).await;
            match res {
                Ok(event_str) => {
                    let cloned_str = event_str.clone();
                    let bytes = cloned_str.into_bytes(); 
                    
                    let res = tx_clone.send(bytes).await; 
                    if let Err(e) = res {
                        log::error!("failed to send event: {:?}", e);
                        break;
                    }
                }
                Err(e) => {
                    log::error!("failed to process scan result: {:?}", e);
                    let payload = serde_json::to_string(&Event {
                        name: "error".to_string(),
                        packet_type: PACKET_TYPE_EVENT.to_string(),
                        data: Some(Value::String(e)),
                    });
                    if let Err(e) = payload {
                        log::error!("failed to serialize error event: {:?}", e);
                        break;
                    }
                    
                    let res = tx_clone.send(payload.unwrap().into_bytes()).await;
                    if let Err(e) = res {
                        log::error!("failed to send error event: {:?}", e);
                    }
                    break;
                }
            }
        }
    });
    tokio::spawn(async move {
        info!("start scan");
        let result = ble_scan.start(10000).await;
        match result {
            Ok(_) => {
                info!("scan finished");
            }
            Err(e) => {
                warn!("scan failed: {:?}", e);
            }
        }
        ble_scan.clear_results();
        listener.abort();
    });

    Ok(Value::String("OK".to_string()))
}

macro_rules! read_param {
    ($params:ident, $name:expr, $type:ty) => {
        match $params.get($name).and_then(|v| v.as_str()) {
            Some(val) => val,
            None => return Err(format!("Parameter '{}' is missing or not a string", $name)),
        }
    };
}

async fn ble_find_client(addr: &str, app_state: &AppState) -> Result<BLEClient, String> {


    let ble_device = BLEDevice::take();
    let ble_scan = ble_device.get_scan();
    let search_res = ble_scan
        .active_scan(true)
        .interval(100)
        .window(99)
        .find_device(5000, |device| device.addr().to_string() == addr)
        .await;
    if let Err(e) = search_res {
        return Err(format!("failed to find device: {:?}", e));
    }

    let device = search_res.unwrap();
    if device.is_none() {
        return Err("device not found".to_string());
    }
    let device = device.unwrap();
    let mut client = BLEClient::new();
    client.on_connect(|client| {
        client.update_conn_params(120, 120, 0, 60).unwrap();
    });
    let res = client.connect(device.addr()).await;
    if let Err(e) = res {
        return Err(format!("failed to connect to device: {:?}", e));
    }
    Ok(client)
}


async fn ble_read_characteristic(params: &Map<String, Value>, app_state: &AppState) -> Result<Value, String> {
    let address = read_param!(params, "address", String);
    let service_uuid = read_param!(params, "service_uuid", String);
    let characteristic_uuid = read_param!(params, "characteristic_uuid", String);


    let uuid_res = BleUuid::from_uuid128_string(&characteristic_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse characteristic uuid: {:?}", e));
    }
    let characteristic_uuid = uuid_res.unwrap();


    let uuid_res = BleUuid::from_uuid128_string(&service_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse service uuid: {:?}", e));
    }
    let service_uuid = uuid_res.unwrap();

    let client_res = ble_find_client(address, app_state).await;


    if let Err(e) = client_res {
        return Err(format!("failed to find client: {:?}", e));
    }
    let mut client = client_res.unwrap();


    let service = client.get_service(service_uuid).await;
    if let Err(e) = service {
        return Err(format!("failed to get service: {:?}", e));
    }
    let service = service.unwrap();

    let characteristic = service.get_characteristic(characteristic_uuid).await;
    if let Err(e) = characteristic {
        return Err(format!("failed to get characteristic: {:?}", e));
    }
    let characteristic = characteristic.unwrap();
    let value = characteristic.read_value().await;
    if let Err(e) = value {
        return Err(format!("failed to read value: {:?}", e));
    }
    let value = value.unwrap();
    Ok(Value::String(hex::encode(value)))
}

async fn ble_write_characteristic(params: &Map<String, Value>, app_state: &AppState) -> Result<Value, String> {
    let address = read_param!(params, "address", String);
    let service_uuid = read_param!(params, "service_uuid", String);
    let characteristic_uuid = read_param!(params, "characteristic_uuid", String);
    let value = read_param!(params, "value", String);

    let uuid_res = BleUuid::from_uuid128_string(&characteristic_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse characteristic uuid: {:?}", e));
    }
    let characteristic_uuid = uuid_res.unwrap();

    let uuid_res = BleUuid::from_uuid128_string(&service_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse service uuid: {:?}", e));
    }
    let service_uuid = uuid_res.unwrap();

    let client_res = ble_find_client(address, app_state).await;
    if let Err(e) = client_res {
        return Err(format!("failed to find client: {:?}", e));
    }
    let mut client = client_res.unwrap();

    let service = client.get_service(service_uuid).await;
    if let Err(e) = service {
        return Err(format!("failed to get service: {:?}", e));
    }
    let service = service.unwrap();

    let characteristic = service.get_characteristic(characteristic_uuid).await;
    if let Err(e) = characteristic {
        return Err(format!("failed to get characteristic: {:?}", e));
    }
    let characteristic = characteristic.unwrap();
    let value = hex::decode(value);
    if let Err(e) = value {
        return Err(format!("failed to decode value: {:?}", e));
    }
    let value = value.unwrap();
    let res = characteristic.write_value(&value, true).await;
    if let Err(e) = res {
        return Err(format!("failed to write value: {:?}", e));
    }
    Ok(Value::String("OK".to_string()))
}

async fn ble_subscribe_characteristic(params: &Map<String, Value>, tx: Sender<Vec<u8>>, app_state: &AppState) -> Result<Value, String> {
    let address = read_param!(params, "address", String);
    let service_uuid = read_param!(params, "service_uuid", String);
    let characteristic_uuid = read_param!(params, "characteristic_uuid", String);

    let uuid_res = BleUuid::from_uuid128_string(&characteristic_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse characteristic uuid: {:?}", e));
    }
    let characteristic_uuid = uuid_res.unwrap();

    let uuid_res = BleUuid::from_uuid128_string(&service_uuid);
    if let Err(e) = uuid_res {
        return Err(format!("failed to parse service uuid: {:?}", e));
    }
    let service_uuid = uuid_res.unwrap();

    let client_res = ble_find_client(address, app_state).await;
    if let Err(e) = client_res {
        return Err(format!("failed to find client: {:?}", e));
    }
    let mut client = client_res.unwrap();

    let service = client.get_service(service_uuid).await;
    if let Err(e) = service {
        return Err(format!("failed to get service: {:?}", e));
    }
    let service = service.unwrap();

    let characteristic = service.get_characteristic(characteristic_uuid).await;
    if let Err(e) = characteristic {
        return Err(format!("failed to get characteristic: {:?}", e));
    }
    let characteristic = characteristic.unwrap();
    if !(characteristic.can_notify()) {
        return Err("characteristic does not support notifications".to_string());
    }


    let address = address.to_string();
    let service_uuid = service_uuid.to_string();
    let characteristic_uuid = characteristic_uuid.to_string();
    let res = characteristic
        .on_notify(move |data| {
            let event = BLENotifyEvent {
                address: address.clone(),
                service_uuid: service_uuid.clone(),
                characteristic_uuid: characteristic_uuid.clone(),
                data: hex::encode(data),
            };
            let event_str_res = serde_json::to_string(&Event {
                name: "ble_notify".to_string(),
                packet_type: PACKET_TYPE_EVENT.to_string(),
                data: Some(event),
            });
            if let Err(e) = event_str_res {
                log::error!("failed to serialize event: {:?}", e);
                return;
            }
            let event_str = event_str_res.unwrap();
            let res = tx.blocking_send(event_str.into_bytes());
            if let Err(e) = res {
                log::error!("failed to send event: {:?}", e);
            }
        })
        .subscribe_notify(false)
        .await;
    if let Err(e) = res {
        return Err(format!("failed to subscribe to notifications: {:?}", e));
    }

    Ok(Value::String("OK".to_string()))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BLENotifyEvent {
    pub address: String,
    pub service_uuid: String,
    pub characteristic_uuid: String,
    pub data: String,
}


pub async fn handle_rpc(payload: &str, tx: Sender<Vec<u8>>, app_state: &AppState) -> serde_json::Result<String> {
    let request_res: std::result::Result<RpcRequest, serde_json::Error> =
        serde_json::from_str(payload);
    if let Err(e) = request_res {
        let response = RpcResponse {
            id: 0,
            packet_type: PACKET_TYPE_RESPONSE.to_string(),
            error: Some(format!("failed to parse request: {}", e)),
            result: None,
        };
        return serde_json::to_string(&response);
    }
    let request = request_res.unwrap();

    let result = match request.method.as_str() {
        "add" => add(&request.params).await,
        "sub" => sub(&request.params).await,
        "get_uuid" => get_uuid().await,
        "bluetooth_start_scan" => bluetooth_start_scan(tx).await,
        "ble_read_characteristic" => ble_read_characteristic(&request.params, app_state).await,
        "ble_write_characteristic" => ble_write_characteristic(&request.params, app_state).await,
        "ble_subscribe_characteristic" => ble_subscribe_characteristic(&request.params, tx, app_state).await,
        "get_info" => get_info().await,
        _ => Err("unknown method".to_string()),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            id: request.id,
            packet_type: PACKET_TYPE_RESPONSE.to_string(),
            error: None,
            result: Some(result),
        },
        Err(e) => RpcResponse {
            id: request.id,
            packet_type: PACKET_TYPE_RESPONSE.to_string(),
            error: Some(e),
            result: None,
        },
    };

    serde_json::to_string(&response)
}
