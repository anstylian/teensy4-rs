//! An adaptation of the `rtic_blink.rs` example that demonstrates logging via Teensy 4 UART.
//!
//! This example requires:
//!
//! - The `rtic` feature to be enabled.
//! - a serial to USB converter (tested with CP2102). The converter should be connected to pins 14
//! and 15. Pin 14 is teensy's TX and pin 15 is teensy's RX pin.
//!
//! Success criteria:
//! - The on-board LED should blink once per second.
//! - On each blink, we receive a message from the teensy via the serial console (e.g. `screen`).
//! - When writing serial data from the console, the teensy should log when each call to the
//! interrupt hardware task occurs and prints the characters received as a utf8 string on each
//! blink.

#![no_std]
#![no_main]

use heapless::consts::U256;
use teensy4_bsp as bsp;
use teensy4_panic as _;

// Type aliases for the Queue we want to use.
type Ty = u8;
type Cap = U256;
type Queue = heapless::spsc::Queue<Ty, Cap>;
type Producer = heapless::spsc::Producer<'static, Ty, Cap>;
type Consumer = heapless::spsc::Consumer<'static, Ty, Cap>;

// The UART receiver.
type UartRx = bsp::hal::uart::Rx<bsp::hal::iomuxc::consts::U2>;

#[rtic::app(device = teensy4_bsp, peripherals = true, dispatchers = [LPUART8])]
mod app {
    use crate::{Consumer, Producer, Queue, UartRx};
    use embedded_hal::digital::v2::OutputPin;
    use embedded_hal::serial::Read;
    use teensy4_bsp as bsp;

    use dwt_systick_monotonic::DwtSystick;
    use rtic::time::duration::Seconds;

    const BAUD: u32 = 115_200;
    const TX_FIFO_SIZE: u8 = 4;

    #[monotonic(binds = SysTick, default = true)]
    type MyMono = DwtSystick<{ bsp::hal::ccm::PLL1::ARM_HZ }>;

    #[resources]
    struct Resources {
        led: bsp::Led,
        u_rx: UartRx,
        q_tx: Producer,
        q_rx: Consumer,
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

        let pins = bsp::t40::into_pins(cx.device.iomuxc);

        // UART setup.
        let uarts = cx.device.uart.clock(
            &mut cx.device.ccm.handle,
            bsp::hal::ccm::uart::ClockSelect::OSC,
            bsp::hal::ccm::uart::PrescalarSelect::DIVIDE_1,
        );
        let mut uart = uarts.uart2.init(pins.p14, pins.p15, BAUD).unwrap();
        uart.set_tx_fifo(core::num::NonZeroU8::new(TX_FIFO_SIZE));
        uart.set_rx_fifo(true);
        uart.set_receiver_interrupt(Some(0));
        let (u_tx, u_rx) = uart.split();
        imxrt_uart_log::blocking::init(u_tx, Default::default()).unwrap();

        // The queue used for buffering bytes.
        static mut Q: Queue = heapless::spsc::Queue(heapless::i::Queue::new());
        let (q_tx, q_rx) = unsafe { Q.split() };

        // LED setup.
        let mut led = bsp::configure_led(pins.p13);
        led.set_high().unwrap();

        // Schedule the first blink.
        blink::spawn_after(Seconds(1_u32)).unwrap();

        (
            init::LateResources {
                led,
                q_rx,
                q_tx,
                u_rx,
            },
            init::Monotonics(mono),
        )
    }

    #[task(resources = [led, q_rx])]
    fn blink(mut cx: blink::Context) {
        // Log via UART.
        static mut TIMES: u32 = 0;
        *TIMES += 1;
        log::info!(
            "`blink` called {} time{}",
            *TIMES,
            if *TIMES > 1 { "s" } else { "" }
        );

        cx.resources.q_rx.lock(|q_rx| {
            if q_rx.ready() {
                let mut buffer = [0u8; 256];
                for elem in buffer.iter_mut() {
                    *elem = match q_rx.dequeue() {
                        None => break,
                        Some(b) => b,
                    };
                }
                let s = core::str::from_utf8(&buffer).unwrap();
                log::info!("read: {}", s);
            }
        });

        // Toggle the LED.
        cx.resources.led.lock(|led| led.toggle());

        // Schedule the following blink.
        blink::spawn_after(Seconds(1_u32)).unwrap();
    }

    #[task(binds = LPUART2, resources = [u_rx, q_tx])]
    fn lpuart2(cx: lpuart2::Context) {
        log::info!("LPUART2 interrupt task called!");
        let u_rx = cx.resources.u_rx;
        let q_tx = cx.resources.q_tx;

        (u_rx, q_tx).lock(|u_rx, q_tx| {
            while let Ok(b) = u_rx.read() {
                q_tx.enqueue(b).ok();
            }
        });
    }
}
