use std::result::Result;
use std::sync::{Arc, Mutex};

use esp32_nimble::{BLEAdvertisedDevice, BLEDevice};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use serde_json::Map;
use tokio::sync::mpsc::Sender;

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub params: Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResponse {
    pub id: u64,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Event<T> {
    pub name: String,
    pub data: Option<T>,
}


#[derive(Serialize, Deserialize, Debug)]
pub struct MyBLEServiceData {
    pub uuid: String,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MyBLEAdvertisedDevice{
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
        data: Some(MyBLEAdvertisedDevice::from(device)),
    };
    let event_str_res = serde_json::to_string(&event);
    if let Err(e) = event_str_res {
        return Err(format!("failed to serialize event: {:?}", e));
    }
    let event_str = event_str_res.unwrap();
    return Ok(event_str);  
}

async fn bluetooth_start_scan(tx: Arc<Sender<String>>) -> Result<Value, String> {
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
                    let res = tx.send(event_str).await;
                    if let Err(e) = res {
                        log::error!("failed to send event: {:?}", e);
                        break;
                    }
                }
                Err(e) => {
                    log::error!("failed to process scan result: {:?}", e);
                    let payload = serde_json::to_string(&Event {
                        name: "error".to_string(),
                        data: Some(Value::String(e)),
                    });
                    let res = tx.send(payload.unwrap()).await;
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

pub async fn handle_rpc(payload: &str, tx: Arc<Sender<String>>) -> serde_json::Result<String> {
    let request_res: std::result::Result<RpcRequest, serde_json::Error> =
        serde_json::from_str(payload);
    if let Err(e) = request_res {
        let response = RpcResponse {
            id: 0,
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
        "get_info" => get_info().await,
        _ => Err("unknown method".to_string()),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            id: request.id,
            error: None,
            result: Some(result),
        },
        Err(e) => RpcResponse {
            id: request.id,
            error: Some(e),
            result: None,
        },
    };

    serde_json::to_string(&response)
}
