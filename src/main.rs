use std::sync::{Arc, Mutex};

use anyhow::Result;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::nvs::EspNvs;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use lazy_static::lazy_static;
use log::{error, info};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
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
    #[default("192.168.1.39")]
    tcp_server_ip: &'static str,
    #[default(12346)]
    tcp_server_port: u16,
}

lazy_static! {
    static ref UUID: Mutex<Uuid> = Mutex::new(Uuid::nil());
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
                ret = tcp_loop() => {
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

async fn tcp_loop() -> Result<()> {
    loop {
        let res = tcp_comm().await;
        if let Err(e) = res {
            error!("TCP loop failed: {:?}", e);
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

async fn tcp_comm() -> Result<(), anyhow::Error> {
    // The constant `CONFIG` is auto-generated by `toml_config`.
    let app_config = CONFIG;
    let sys_loop = peripherals::SYS_LOOP.clone();
    let connect_res = TcpStream::connect(format!(
        "{}:{}",
        app_config.tcp_server_ip, app_config.tcp_server_port
    ))
    .await;
    if let Err(e) = connect_res {
        error!("Failed to connect to TCP server: {:?}", e);
        return Err(anyhow::Error::from(e));
    }
    let (tcp_rx, mut tcp_tx): (
        tokio::net::tcp::OwnedReadHalf,
        tokio::net::tcp::OwnedWriteHalf,
    ) = connect_res.unwrap().into_split();



    // multi producer single consumer channel
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(100);

    let _ = tx.send("Hello from ESP32".to_string()).await;
    info!("Waiting for response from TCP server");

    tokio::select! {
        _ = tcp_comm_loop_handle_read(tcp_rx, tx) => {
            error!("TCP read loop finished");
        },
        _ = tcp_comm_loop_handle_write(tcp_tx, rx) => {
            error!("TCP write loop finished");
        }
    }

    Err(anyhow::Error::msg("TCP loop finished"))
}

async fn tcp_comm_loop_handle_write(
    mut stream: tokio::net::tcp::OwnedWriteHalf,
    mut rx: tokio::sync::mpsc::Receiver<String>,
) -> Result<()> {
    loop {
        let message = rx.recv().await;
        if let Some(message) = message {
            let bytes = message.as_bytes();

            let mut bytes = bytes.to_vec();
            //add newline
            bytes.push(10);

            let _ = stream.write_all(&bytes).await;
        }
    }
}

async fn tcp_comm_loop_handle_read(
    mut stream: tokio::net::tcp::OwnedReadHalf,
    tx: tokio::sync::mpsc::Sender<String>,
) -> Result<()> {
    let mut reader = tokio::io::BufReader::new(stream);

    loop {
        let mut string = String::new();
        let read_res = reader.read_line(&mut string).await;
        if let Err(e) = read_res {
            error!("Failed to read from TCP server: {:?}", e);
            return Err(anyhow::Error::from(e));
        }
        let try_as_string = string.trim_end().to_string();

        info!("Got response from TCP server: {:?}", try_as_string);
        let cloned_tx = tx.clone();
        let tx_clone = Arc::new(cloned_tx);
        tokio::spawn(async move {
            let res = rpc::handle_rpc(&try_as_string, tx_clone.clone()).await;
            if let Err(e) = res {
                let message = format!("Failed to handle rpc: {:?}", e);
                let _ = tx_clone.send(message.clone()).await;
                error!("{}", message);
            } else {
                let message = res.unwrap();
                let _ = tx_clone.send(message.clone()).await;
                info!("Sent response to TCP server: {:?}", message);
            }
        });
    }
    
}

async fn led_loop<T: esp_idf_hal::gpio::Pin>(mut led_blue: PinDriver<'_, T, Output>) -> Result<()> {
    loop {
        // allow syncronizhed blinking with other ESP32 without communication
        // % 5 == 0
        let current = std::time::SystemTime::now();
        let since_the_epoch = current.duration_since(std::time::UNIX_EPOCH).unwrap();
        let secs = since_the_epoch.as_millis();
        // sleep until the next 5 second mark
        let sleep_time = 5000 - (secs % 5000);
        let sleep_time = std::time::Duration::from_millis(sleep_time as u64);
        tokio::time::sleep(sleep_time).await;

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
