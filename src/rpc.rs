use tokio::net::TcpStream;
use serde::{Deserialize, Serialize};
use serde_json::{Map};
use crate::preludes::*;


#[derive(Serialize, Deserialize, Debug)]
pub struct RpcRequest {
    pub method: String,
    pub params: Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcResponse {
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}


use serde_json::{ Value, Number};
use std::result::Result;

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
 
async fn bluetooth_start_scan() -> Result<Value, String> {
    Ok(Value::String("Scanning".to_string()))
}

pub async fn handle_rpc(
    payload: &str,
) -> serde_json::Result<String> {
 
    




    let request_res: std::result::Result<RpcRequest, serde_json::Error> = serde_json::from_str(payload);
    if let Err(e) = request_res {
        let response = RpcResponse {
            error: Some(format!("failed to parse request: {}", e)),
            result: None,
        };
        return serde_json::to_string(&response);
    }
    let request = request_res.unwrap();
    
    let result = match request.method.as_str() {
        "add" => add(&request.params).await,
        "sub" => sub(&request.params).await,
        _ => Err("unknown method".to_string()),
    };
    
    let response = RpcResponse {
        error: None,
        result: result.ok(),
    };
    serde_json::to_string(&response)
}
