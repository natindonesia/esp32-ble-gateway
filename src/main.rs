use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use embedded_svc::mqtt::client::Client;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp32_nimble::BLEClient;
use esp_idf_hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::mqtt::client::{
    EspAsyncMqttClient, EspAsyncMqttConnection, EspMqttClient, EspMqttEvent, EventPayload,
    MqttClientConfiguration,
};
use esp_idf_svc::nvs::EspNvs;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use esp_idf_sys::EspError;
use lazy_static::lazy_static;
use log::{error, info};
use rpc::{Event, RpcResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

mod ntp;
mod peripherals;
mod preludes;
mod rpc;
mod wifi;

/// This configuration is picked up at compile time by `build.rs` from the
/// file `cfg.toml`.
#[toml_cfg::toml_config]
pub struct Config {
    #[default("Wokwi-GUEST")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
    #[default("mqtt://192.168.1.8:1883")]
    mqtt_endpoint: &'static str,
}

const MQTT_TOPIC_RECEIVE: &str = "esp32-ble-proxy/devices";

#[derive(Default)]
pub struct AppState {
    pub ble_clients: Vec<Arc<Mutex<BLEClient>>>,
    pub ble_scan_running: Arc<Mutex<bool>>,
}

lazy_static! {
    static ref UUID: Mutex<Uuid> = Mutex::new(Uuid::nil());
    static ref MQTT_TOPIC_SEND: Mutex<String> = Mutex::new("".to_string()); // esp32-ble-proxy/devices/UUID
}

/// Entry point to our application.
///
/// It sets up a Wi-Fi connection to the Access Point given in the
/// configuration, then blinks the RGB LED green/blue.
///
/// If the LED goes solid red, then it was unable to connect to your Wi-Fi
/// network.
///

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    // `async-io` uses the ESP IDF `eventfd` syscall to implement async IO.
    // If you use `tokio`, you still have to do the same as it also uses the `eventfd` syscall
    esp_idf_svc::io::vfs::initialize_eventfd(5).unwrap();

    let sys_loop = peripherals::SYS_LOOP.clone();
    let timer_service = peripherals::ESP_TASK_TIMER_SVR.clone();
    let nvs_partition = peripherals::NVS_DEFAULT_PARTITION.clone();

    // get uuid or make one
    let nvs_namespace = "main";
    let mut nvs = match EspNvs::new(nvs_partition.clone(), nvs_namespace, true) {
        Ok(nvs) => {
            info!("Got namespace {:?} from default partition", nvs_namespace);
            nvs
        }
        Err(e) => anyhow::bail!("Could't get namespace {:?}", e),
    };

    const MAX_STR_LEN: usize = 128;
    // check if uuid exists
    let the_str_len = nvs.str_len("uuid").map_or(0, |v| {
        info!("Got stored string length of {:?}", v);
        let vv = v.unwrap_or(0);
        if vv >= MAX_STR_LEN {
            error!("Too long, trimming");
            0
        } else {
            vv
        }
    });

    let uuid;
    if the_str_len > 0 {
        let mut buffer_uuid: [u8; MAX_STR_LEN] = [0; MAX_STR_LEN];
        let uuid_option = match nvs.get_str("uuid", &mut buffer_uuid) {
            Ok(uuid) => {
                info!("Got uuid from NVS: {:?}", uuid);
                uuid
            }
            Err(e) => {
                anyhow::bail!("Couldn't get uuid from NVS: {:?}", e);
            }
        };
        if uuid_option.is_none() {
            anyhow::bail!("Couldn't get uuid from NVS");
        }

        let uuid_res = Uuid::parse_str(uuid_option.unwrap());
        if uuid_res.is_err() {
            anyhow::bail!("Couldn't parse uuid from NVS: {:?}", uuid_res);
        }
        uuid = uuid_res.unwrap();
    } else {
        info!("No uuid found in NVS");
        let new_uuid = Uuid::new_v4();
        let new_uuid_str = new_uuid.clone().to_string();
        info!("Generated new uuid: {:?}", new_uuid_str.clone());
        let res = nvs.set_str("uuid", new_uuid_str.as_str());
        if res.is_err() {
            anyhow::bail!("Couldn't set uuid in NVS: {:?}", res);
        }
        uuid = new_uuid;
    }

    // Add the UUID to the global static
    *UUID.lock().unwrap() = uuid;
    *MQTT_TOPIC_SEND.lock().unwrap() = format!("{}/{}", MQTT_TOPIC_RECEIVE, uuid.to_string());

    info!("------------------------------------");
    info!("UUID: {:?}", uuid);
    info!("------------------------------------");

    info!("Hello, world!");

    // Start the LED
    let mut led_blue = peripherals::take_gpio2_output();
    let _ = led_blue.set_high();

    let mut wifi = AsyncWifi::wrap(peripherals::create_esp_wifi(), sys_loop, timer_service)?;

    let mut res = block_on(connect_wifi(&mut wifi));
    while let Err(e) = res {
        log::error!("Failed to connect to wifi: {:?}", e);
        log::info!("Retrying in 5 seconds...");
        let current_time = std::time::Instant::now();
        //go blinking red
        loop {
            const WAIT: std::time::Duration = std::time::Duration::from_millis(150);
            let _ = led_blue.set_low();
            // Wait...
            std::thread::sleep(WAIT);
            let _ = led_blue.set_high();
            // Wait...
            std::thread::sleep(WAIT);
            // check if 5 seconds have passed
            if current_time.elapsed().as_secs() >= 5 {
                break;
            }
        }
        res = block_on(connect_wifi(&mut wifi));
    }

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    info!("Wifi DHCP info: {:?}", ip_info);

    let ntp_res = ntp::ntp_sync();
    if ntp_res.is_err() {
        error!("Failed to sync time: {:?}", ntp_res);
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            info!("Starting loops");

            tokio::select! {

                ret = led_loop(led_blue) => {
                    if let Err(e) = ret {
                        error!("LED loop failed: {:?}", e);
                    }
                    error!("LED loop finished");
                },
                ret = wifi::app_wifi_loop(wifi) => {
                    if let Err(e) = ret {
                        error!("Wifi loop failed: {:?}", e);
                    }
                    error!("Wifi loop finished");
                },
                ret = mqtt_loop() => {
                    if let Err(e) = ret {
                        error!("TCP loop failed: {:?}", e);
                    }
                    error!("TCP loop finished");
                }
            }
            error!("All loops finished");
        });

    Ok(())
}

async fn init_mqtt(client: &mut EspMqttClient<'_>) {
    loop {
        let topic = MQTT_TOPIC_RECEIVE;
        log::info!("Subscribing to topic: {:?}", topic);
        let res = client.subscribe(
            MQTT_TOPIC_RECEIVE,
            esp_idf_svc::mqtt::client::QoS::AtLeastOnce,
        );
        if res.is_err() {
            log::error!("Failed to subscribe to topic: {:?}", res.err());
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        break;
    }
    loop {
        let topic = format!("{}/rpc", MQTT_TOPIC_SEND.lock().unwrap().clone());
        log::info!("Subscribing to topic: {:?}", topic);
        let res = client.subscribe(topic.as_str(), esp_idf_svc::mqtt::client::QoS::ExactlyOnce);
        if res.is_err() {
            log::error!("Failed to subscribe to topic: {:?}", res.err());
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        break;
    }
    let mut register_event: Event<String> = Event::default();
    register_event.name = "register".to_string();
    register_event.data = Some(UUID.lock().unwrap().to_string());
    let payload_register_message = serde_json::to_string(&register_event).unwrap();
    loop {
        log::info!(
            "Sending register message to: {:?}",
            MQTT_TOPIC_SEND.lock().unwrap()
        );
        let res = client.publish(
            MQTT_TOPIC_SEND.lock().unwrap().as_str(),
            esp_idf_svc::mqtt::client::QoS::AtLeastOnce,
            false,
            payload_register_message.as_bytes(),
        );
        if res.is_err() {
            log::error!("Failed to send register message: {:?}", res.err());
            continue;
        }
        break;
    }

    info!("Sent hello message");
}

async fn send_loop_mqtt(
    client: &mut EspMqttClient<'_>,
    mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    loop {
        let data = rx.recv().await;
        if data.is_none() {
            log::error!("Failed to get data from channel");
            continue;
        }
        let data = data.unwrap();

        // check if before connect
        let res = std::str::from_utf8(&data);
        if res.is_ok() {
            let string = res.unwrap();
            if string == "BeforeConnect" {
                init_mqtt(client).await;
                continue;
            }
        }

        let res = client.publish(
            MQTT_TOPIC_SEND.lock().unwrap().as_str(),
            esp_idf_svc::mqtt::client::QoS::ExactlyOnce,
            false,
            data.as_slice(),
        );
        if res.is_err() {
            log::error!("Failed to send data: {:?}", res.err());
            continue;
        }
    }
}

async fn mqtt_loop() -> Result<()> {
    let app_config = CONFIG;
    let device_uuid = UUID.lock().unwrap().clone();
    let formatted_string = format!("esp32-ble-proxy-{}", device_uuid.to_string());
    let client_id = Some(formatted_string.as_str());

    let mqtt_config = MqttClientConfiguration {
        client_id,
        ..Default::default()
    };

    let res = EspMqttClient::new(app_config.mqtt_endpoint, &mqtt_config);
    if res.is_err() {
        log::error!("Failed to create mqtt client: {:?}", res.err());
        return Ok(());
    }
    let (mut client, con) = res.unwrap();

    let workload = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let workload_copy = workload.clone();
    let thread_handle = std::thread::spawn(move || {
        let mut connection = con;
        loop {
            let event = connection.next();
            if event.is_err() {
                log::error!("Failed to get event: {:?}", event.err());
                continue;
            }
            let event = event.unwrap();
            let payload = event.payload();
            match payload {
                EventPayload::Received {
                    id,
                    topic,
                    data,
                    details,
                    ..
                } => {
                    match details {
                        esp_idf_svc::mqtt::client::Details::Complete => {
                            // good
                        }
                        _ => {
                            log::error!("MQTT: {:?}", details);
                            continue;
                        }
                    }
                    workload_copy.lock().unwrap().push(data.to_vec());
                    log::info!("MQTT: {:?}", topic);
                }
                EventPayload::BeforeConnect => {
                    workload_copy
                        .lock()
                        .unwrap()
                        .push(b"BeforeConnect".to_vec());
                }
                _ => {
                    log::info!("MQTT: {:?}", payload);
                }
            }
        }

        error!("MQTT Connection closed");
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(10);
    let _ = tokio::spawn(async move { return send_loop_mqtt(&mut client, rx).await });
    let mut app_state = AppState::default();
    loop {
        let mut workload: std::sync::MutexGuard<Vec<Vec<u8>>> = workload.lock().unwrap();
        if workload.len() > 0 {
            let data = workload.remove(0);
            let res = std::str::from_utf8(&data);
            if res.is_err() {
                log::error!("Failed to convert data to string: {:?}", res.err());
                continue;
            }
            let string = res.unwrap();
            log::info!("Got data: {:?}", string);

            if string == "BeforeConnect" {
                loop {
                    let res = tx.send(b"BeforeConnect".to_vec()).await;
                    if res.is_err() {
                        log::error!("Failed to send BeforeConnect: {:?}", res.err());
                        continue;
                    }
                    break;
                }
                continue;
            }

            let res = rpc::handle_rpc(&string, tx.clone(), &mut app_state).await;
            if res.is_err() {
                let error = res.err().unwrap();
                log::error!("Failed to handle rpc: {:?}", error);
                let mut error = RpcResponse::default();
                error.error = Some(format!("{:?}", error));
                let res = tx.send(serde_json::to_vec(&error).unwrap()).await;
                if res.is_err() {
                    log::error!("Failed to send error: {:?}", res.err());
                }
            } else if res.is_ok() {
                let send_res = tx.send(res.unwrap().into_bytes()).await;
                if send_res.is_err() {
                    log::error!("Failed to send response: {:?}", send_res.err());
                }
                log::info!("Sent response: {:?}", string);
            }
        }
        // check if thread died
        if thread_handle.is_finished() {
            log::error!("Thread died :(");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let res = thread_handle.join();
    if res.is_err() {
        log::error!("Failed to join thread: {:?}", res.err());
    }
    Ok(())
}

async fn led_loop<T: esp_idf_hal::gpio::Pin>(mut led_blue: PinDriver<'_, T, Output>) -> Result<()> {
    // allow synchronized blinking with other ESP32 without communication
    // % 5 == 0
    let current = std::time::SystemTime::now();
    let since_the_epoch = current.duration_since(std::time::UNIX_EPOCH).unwrap();
    let secs = since_the_epoch.as_millis();
    // sleep until the next 5-second mark
    let sleep_time = 5000 - (secs % 5000);
    let sleep_time = std::time::Duration::from_millis(sleep_time as u64);
    tokio::time::sleep(sleep_time).await;
    loop {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000);
        let _ = led_blue.set_low();
        tokio::time::sleep(INTERVAL).await;

        let _ = led_blue.set_high();
        tokio::time::sleep(INTERVAL).await;
    }
}

async fn connect_wifi(wifi: &mut AsyncWifi<EspWifi<'static>>) -> anyhow::Result<()> {
    // The constant `CONFIG` is auto-generated by `toml_config`.
    let app_config = CONFIG;
    let auth_method = if app_config.wifi_psk.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    info!(
        "Connecting to wifi: {:?} with auth method: {:?}",
        app_config.wifi_ssid, auth_method
    );
    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: app_config.wifi_ssid.try_into().unwrap(),
        bssid: None,
        auth_method: auth_method,
        password: app_config.wifi_psk.try_into().unwrap(),
        channel: None,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;

    wifi.start().await?;
    info!("Wifi started");

    wifi.connect().await?;
    info!("Wifi connected");

    wifi.wait_netif_up().await?;
    info!("Wifi netif up");

    Ok(())
}
