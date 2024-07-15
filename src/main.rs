use anyhow::Result;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_hal::gpio::{Output, PinDriver};

use esp_idf_svc::nvs::EspNvs;
use log::{error, info};

use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use futures_util::{SinkExt, StreamExt};
use uuid::Uuid;

mod ntp;
mod peripherals;
mod preludes;
mod wifi;

/// This configuration is picked up at compile time by `build.rs` from the
/// file `cfg.toml`.
#[toml_cfg::toml_config]
pub struct Config {
    #[default("Wokwi-GUEST")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
    #[default("wss://echo.websocket.org")]
    websocket_url: &'static str,
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
    peripherals::patch_eventfd();

    

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

    info!("------------------------------------");
    info!("UUID: {:?}", uuid);
    info!("------------------------------------");

    info!("Hello, world!");

    // Start the LED
    let mut led_blue = peripherals::take_gpio2_output();
    let _ = led_blue.set_high();

    

    let mut wifi = AsyncWifi::wrap(
        peripherals::create_esp_wifi(),
        sys_loop,
        timer_service,
    )?;

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
                ret = websocket_loop() => {
                    if let Err(e) = ret {
                        error!("Websocket loop failed: {:?}", e);
                    }
                    error!("Websocket loop finished");
                },
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
            }
            error!("All loops finished");
        });

    Ok(())
}



async fn websocket_loop() -> Result<()> {
    // The constant `CONFIG` is auto-generated by `toml_config`.
    let app_config = CONFIG;
    info!("Connecting to websocket: {:?}", app_config.websocket_url);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    Ok(())
}

async fn led_loop<T: esp_idf_hal::gpio::Pin>(mut led_blue: PinDriver<'_, T, Output>) -> Result<()> {
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
