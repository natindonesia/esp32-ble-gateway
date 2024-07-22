use std::borrow::Borrow;
use std::result::Result;

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
pub struct Event {
    pub name: String,
    pub data: Option<serde_json::Value>,

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
    map.insert("heap_caps_get_free_size".to_string(), Value::Number(Number::from(
        heap_caps_get_free_size
    )));
    let esp_get_minimum_free_heap_size = unsafe { esp_idf_sys::esp_get_minimum_free_heap_size() };
    map.insert("esp_get_minimum_free_heap_size".to_string(), Value::Number(Number::from(
        esp_get_minimum_free_heap_size
    )));
    let esp_get_free_heap_size = unsafe { esp_idf_sys::esp_get_free_heap_size() };
    map.insert("esp_get_free_heap_size".to_string(), Value::Number(Number::from(
        esp_get_free_heap_size
    )));
    



    Ok(Value::Object(map))
}

async fn on_ble_scan_result(device: BLEAdvertisedDevice, tx: Arc<Sender<String>>){

    let mut map: Map<String, Value> = Map::new();
    map.insert("name".to_string(), Value::String(device.name().to_string()));
    map.insert("address".to_string(), Value::String(device.addr().to_string()));
    map.insert("rssi".to_string(), Value::Number(Number::from(device.rssi())));
    map.insert("service_uuids".to_string(), Value::Array(device.get_service_uuids().map(|uuid| Value::String(uuid.to_string())).collect()));
    map.insert("service_data".to_string(), Value::Array(device.get_service_data_list().map(|data| {
        let mut map = Map::new();
        map.insert("uuid".to_string(), Value::String(data.uuid().to_string()));
        map.insert("data".to_string(), Value::Array(data.data().iter().map(|b| Value::Number(Number::from(*b))).collect()));
        Value::Object(map)
    }).collect()));
    let manufacture_data: Option<&[u8]> = device.get_manufacture_data();
    if let Some(data) = manufacture_data {
        map.insert("manufacture_data".to_string(), Value::Array(data.iter().map(|b| Value::Number(Number::from(*b))).collect()));
    }

    map.insert("adv_type".to_string(), Value::String(std::format!("{:?}", device.adv_type())));
    let adv_flags: Option<esp32_nimble::enums::AdvFlag> = device.adv_flags();
    if let Some(flag) = adv_flags {
        map.insert("adv_flags".to_string(), Value::String(std::format!("{:?}", flag)));
    }

    map.insert("raw_data".to_string(), Value::Array(device.raw_data.iter().map(|b| Value::Number(Number::from(*b))).collect()));
    let event = Event {
        name: "ble_scan_result".to_string(),
        data: Some(Value::Object(map)),
    };
    let event_str_res = serde_json::to_string(&event);
    if let Err(e) = event_str_res {
        log::error!("failed to serialize event: {}", e);
        let event = Event {
            name: "error".to_string(),
            data: Some(Value::String("failed to serialize event".to_string())),
        };
        let event_str_res = serde_json::to_string(&event);
        if let Err(e) = event_str_res {
            log::error!("failed to serialize error event: {}", e);
            return;
        }
        tx.send(event_str_res.unwrap()).await.unwrap();
        return;
    }
    let event_str = event_str_res.unwrap();

    tx.send(event_str).await.unwrap();
}

use std::sync::Arc;

async fn bluetooth_start_scan(tx: Arc<Sender<String>>) -> Result<Value, String> {
    let ble_device = BLEDevice::take();
    let ble_scan = ble_device.get_scan();
    
    ble_scan
      .active_scan(true)
      .interval(100)
      .window(99)
      .on_result(move |_scan, param, | {
          let tx: Arc<Sender<String>> = Arc::clone(&tx);
          
          tokio::spawn(on_ble_scan_result(param.clone(), tx));
      });
    
    tokio::spawn(async move {
        let result = ble_scan.start(10000).await;
        match result {
            Ok(_) => {
                info!("scan finished");
            }
            Err(e) => {
                warn!("scan failed: {:?}", e);
            }
        }
    });

    Ok(Value::Null)
}

pub async fn handle_rpc(
    payload: &str,
    tx: Arc<Sender<String>>
) -> serde_json::Result<String> {
 
    




    let request_res: std::result::Result<RpcRequest, serde_json::Error> = serde_json::from_str(payload);
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
