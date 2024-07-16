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


async fn add(params: &Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
    let a = params.get("a").unwrap().as_i64().unwrap();
    let b = params.get("b").unwrap().as_i64().unwrap();
    Ok(serde_json::Value::Number(serde_json::Number::from(a + b)))
}

async fn sub(params: &Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
    let a = params.get("a").unwrap().as_i64().unwrap();
    let b = params.get("b").unwrap().as_i64().unwrap();
    Ok(serde_json::Value::Number(serde_json::Number::from(a - b)))
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
