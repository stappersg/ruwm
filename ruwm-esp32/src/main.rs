#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]
use std::env;

extern crate alloc;
use alloc::sync::Arc;

use embedded_graphics::prelude::{Point, RgbColor, Size};
use embedded_graphics::primitives::Rectangle;

use display_interface_spi::SPIInterfaceNoCS;

use embedded_hal::digital::v2::OutputPin;

use embedded_svc::event_bus::nonblocking::EventBus;
use embedded_svc::utils::nonblocking::Asyncify;
use embedded_svc::wifi::{ClientConfiguration, Configuration, Wifi};
use embedded_svc::ws::server::registry::Registry;

use esp_idf_hal::gpio::{Output, Pull};
use esp_idf_hal::mutex::Mutex;
use esp_idf_hal::prelude::*;
use esp_idf_hal::spi::SPI2;
use esp_idf_hal::{adc, delay, gpio, spi};

use esp_idf_svc::http::server::ws::EspHttpWsProcessor;
use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::EspWifi;

use pulse_counter::PulseCounter;

use ruwm::broadcast_binder;
use ruwm::error;
use ruwm::pulse_counter::PulseCounter as _;
use ruwm::screen::{CroppedAdaptor, FlushableAdaptor, FlushableDrawTarget};

use ruwm_std::unblocker::SmolUnblocker;

#[cfg(feature = "espidf")]
use crate::espidf::*;

#[cfg(not(feature = "espidf"))]
use ruwm_std::*;

#[cfg(feature = "espidf")]
mod espidf;
#[cfg(any(esp32, esp32s2))]
mod pulse_counter;

const SSID: &str = env!("RUWM_WIFI_SSID");
const PASS: &str = env!("RUWM_WIFI_PASS");

fn main() -> error::Result<()> {
    init()?;

    let peripherals = Peripherals::take().unwrap();

    #[cfg(feature = "espidf")]
    let broadcast = broadcast::broadcast::<espidf::broadcast_event_serde::Serde, _>(100)?;

    #[cfg(not(feature = "espidf"))]
    let broadcast = broadcast::broadcast(100)?;

    let binder = broadcast_binder::BroadcastBinder::<
        SmolUnblocker,
        Mutex<_>,
        Mutex<_>,
        Mutex<_>,
        _,
        _,
        _,
        _,
        _,
    >::new(broadcast, timer::timers()?, signal::SignalFactory);

    let netif_stack = Arc::new(EspNetifStack::new()?);
    let sysloop_stack = Arc::new(EspSysLoopStack::new()?);
    let nvs_stack = Arc::new(EspDefaultNvs::new()?);

    let mut wifi = EspWifi::new(netif_stack, sysloop_stack, nvs_stack)?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: SSID.into(),
        password: PASS.into(),
        ..Default::default()
    }))?;

    let (web_processor, web_acceptor) = EspHttpWsProcessor::new::<SmolUnblocker>();

    let web_processor = esp_idf_hal::mutex::Mutex::new(web_processor);

    let mut httpd = EspHttpServer::new(&Default::default())?;

    httpd
        .ws("/ws")
        .handler(move |receiver, sender| web_processor.lock().process(receiver, sender))?;

    let client_id = "water-meter-demo";

    let binder = binder
        .event_logger()?
        .wifi(wifi.as_async().subscribe()?)?
        .valve(
            peripherals.pins.gpio10.into_output()?,
            peripherals.pins.gpio12.into_output()?,
            peripherals.pins.gpio13.into_output()?,
        )?
        .battery(
            adc::PoweredAdc::new(
                peripherals.adc1,
                adc::config::Config::new().calibration(true),
            )?,
            peripherals.pins.gpio35.into_analog_atten_11db()?,
            peripherals.pins.gpio14.into_input()?,
        )?
        .water_meter(PulseCounter::new(peripherals.ulp).initialize()?)?
        .button(
            1,
            "BUTON1",
            peripherals.pins.gpio25.into_input()?.into_pull_up()?,
        )?
        .button(
            2,
            "BUTON2",
            peripherals.pins.gpio26.into_input()?.into_pull_up()?,
        )?
        .button(
            3,
            "BUTON3",
            peripherals.pins.gpio27.into_input()?.into_pull_up()?,
        )?
        .screen(display(
            peripherals.pins.gpio4.into_output()?.degrade(),
            peripherals.pins.gpio16.into_output()?.degrade(),
            peripherals.pins.gpio23.into_output()?.degrade(),
            peripherals.spi2,
            peripherals.pins.gpio18.into_output()?.degrade(),
            peripherals.pins.gpio19.into_output()?.degrade(),
            Some(peripherals.pins.gpio5.into_output()?.degrade()),
        )?)?
        .mqtt(
            client_id,
            EspMqttClient::new_async::<SmolUnblocker, _>(
                "mqtt://broker.emqx.io:1883",
                MqttClientConfiguration {
                    client_id: Some(client_id),
                    ..Default::default()
                },
            )?,
        )?
        .web::<_, esp_idf_hal::mutex::Mutex<_>>(web_acceptor)?
        .emergency()?;

    smol::block_on(binder.into_future())?;

    Ok(())
}

fn init() -> error::Result<()> {
    env::set_var("BLOCKING_MAX_THREADS", "2");

    esp_idf_sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    esp_idf_sys::esp!(unsafe {
        #[allow(clippy::needless_update)]
        esp_idf_sys::esp_vfs_eventfd_register(&esp_idf_sys::esp_vfs_eventfd_config_t {
            max_fds: 5,
            ..Default::default()
        })
    })?;

    Ok(())
}

fn display(
    mut backlight: gpio::GpioPin<Output>,
    dc: gpio::GpioPin<Output>,
    rst: gpio::GpioPin<Output>,
    spi: SPI2,
    sclk: gpio::GpioPin<Output>,
    sdo: gpio::GpioPin<Output>,
    cs: Option<gpio::GpioPin<Output>>,
) -> error::Result<impl FlushableDrawTarget<Color = impl RgbColor, Error = impl core::fmt::Debug>> {
    backlight.set_high()?;

    let di = SPIInterfaceNoCS::new(
        spi::Master::<SPI2, _, _, _, _>::new(
            spi,
            spi::Pins {
                sclk,
                sdo,
                sdi: Option::<gpio::Gpio21<gpio::Unknown>>::None,
                cs,
            },
            <spi::config::Config as Default>::default().baudrate(26.MHz().into()),
        )?,
        dc,
    );

    let mut display = st7789::ST7789::new(
        di, rst,
        // SP7789V is designed to drive 240x320 screens, even though the TTGO physical screen is smaller
        240, 320,
    );

    display.init(&mut delay::Ets).unwrap();
    display
        .set_orientation(st7789::Orientation::Portrait)
        .unwrap();

    // The TTGO board's screen does not start at offset 0x0, and the physical size is 135x240, instead of 240x320
    let display = FlushableAdaptor::noop(CroppedAdaptor::new(
        Rectangle::new(Point::new(52, 40), Size::new(135, 240)),
        display,
    ));

    Ok(display)
}
