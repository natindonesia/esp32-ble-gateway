use tokio::net::TcpStream;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Result};
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

pub async fn handle_rpc(
    payload: &str,
) -> Result<String> {
    let request_res: std::result::Result<RpcRequest, serde_json::Error> = serde_json::from_str(payload);
    if let Err(e) = request_res {
        let response = RpcResponse {
            error: Some(format!("failed to parse request: {}", e)),
            result: None,
        };
        return serde_json::to_string(&response);
    }
    let request = request_res.unwrap();
    let result: Option<serde_json::Value> = match request.method.as_str() {
        "add" => {
            let a = request.params.get("a").unwrap().as_i64().unwrap();
            let b = request.params.get("b").unwrap().as_i64().unwrap();
            Some(serde_json::Value::Number(serde_json::Number::from(a + b)))
        }
        "sub" => {
            let a = request.params.get("a").unwrap().as_i64().unwrap();
            let b = request.params.get("b").unwrap().as_i64().unwrap();
            Some(serde_json::Value::Number(serde_json::Number::from(a - b)))
        }
        _ => {
            let response = RpcResponse {
                error: Some(format!("unknown method: {}", request.method)),
                result: None,
            };
            return serde_json::to_string(&response);
        }
    };
    let response = RpcResponse {
        error: None,
        result: result,
    };
    serde_json::to_string(&response)
}
