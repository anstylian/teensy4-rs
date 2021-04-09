//! An adaptation of the `rtic_blink.rs` example that adds USB logging.
//!
//! The logging support is handled by the BSP. Specifically, the BSP manages the
//! USB interrupt, and configures the peripheral for high-speed USB CDC support.
//! Simply use the logging macros in your RTIC application to send data to your
//! USB host.
//!
//! Please refer to the [RTIC book](https://rtic.rs) for more information on RTIC.
//!
//! NOTE: This example requires the `rtic` and `usb-logging` features to be enabled.

#![no_std]
#![no_main]

use teensy4_panic as _;

#[rtic::app(device = teensy4_bsp, peripherals = true, dispatchers = [LPUART8])]
mod app {
    use bsp::hal::ral::usb::USB1;
    use embedded_hal::digital::v2::OutputPin;
    use teensy4_bsp as bsp;

    use dwt_systick_monotonic::DwtSystick;
    use rtic::time::duration::Seconds;

    #[monotonic(binds = SysTick, default = true)]
    type MyMono = DwtSystick<{ bsp::hal::ccm::PLL1::ARM_HZ }>;

    #[resources]
    struct Resources {
        led: bsp::Led,
        poller: bsp::usb::Poller,
    }

    #[init()]
    fn init(mut cx: init::Context) -> (init::LateResources, init::Monotonics) {
        let mut dcb = cx.core.DCB;
        let dwt = cx.core.DWT;
        let systick = cx.core.SYST;

        let mono = DwtSystick::new(&mut dcb, dwt, systick, bsp::hal::ccm::PLL1::ARM_HZ);

        cx.device.ccm.pll1.set_arm_clock(
            bsp::hal::ccm::PLL1::ARM_HZ,
            &mut cx.device.ccm.handle,
            &mut cx.device.dcdc,
        );

        // Schedule the first blink.
        blink::spawn_after(Seconds(1_u32)).unwrap();
        let pins = bsp::t40::into_pins(cx.device.iomuxc);
        let mut led = bsp::configure_led(pins.p13);
        led.set_high().unwrap();

        // Initialize the USB system
        let (poller, _) = bsp::usb::init(USB1::take().unwrap(), Default::default()).unwrap();

        (init::LateResources { led, poller }, init::Monotonics(mono))
    }

    #[task(resources = [led])]
    fn blink(mut cx: blink::Context) {
        static mut COUNTER: u32 = 0;
        cx.resources.led.lock(|led| led.toggle());
        // Schedule the following blink.
        blink::spawn_after(Seconds(1_u32)).unwrap();
        log::info!("Hello from RTIC! Count = {}", *COUNTER);
        *COUNTER += 1;
    }

    #[task(binds = USB_OTG1, resources = [poller])]
    fn usb_otg1(mut cx: usb_otg1::Context) {
        cx.resources.poller.lock(|poller| poller.poll());
    }
}
