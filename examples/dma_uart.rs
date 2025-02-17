//! Demonstrates a DMA-based UART transfer and receive
//!
//! The example waits for a UART character, then echos that
//! character back to the user with a header. Each received
//! character causes the LED to toggle. Received bytes and
//! replies are logged over USB.
//!
//! Pinout:
//! - Pin 14: UART2_TX
//! - Pin 15: UART2_RX

#![no_std]
#![no_main]

mod usb_io;

use teensy4_panic as _;

use bsp::interrupt;
use cortex_m_rt::entry;
use teensy4_bsp as bsp;

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use cortex_m::interrupt::{free, Mutex};

/// Modify your baud rate here
const BAUD: u32 = 115_200;

type TxBuffer = bsp::hal::dma::Linear<u8>;
type RxBuffer = bsp::hal::dma::Circular<u8>;

#[repr(align(64))]
struct Align64(bsp::hal::dma::Buffer<[u8; 64]>);

static TX_MEM: bsp::hal::dma::Buffer<[u8; 64]> = bsp::hal::dma::Buffer::new([0; 64]);
static RX_MEM: Align64 = Align64(bsp::hal::dma::Buffer::new([0; 64]));

static TX_BUFFER: Mutex<RefCell<Option<TxBuffer>>> = Mutex::new(RefCell::new(None));
static RX_BUFFER: Mutex<RefCell<Option<RxBuffer>>> = Mutex::new(RefCell::new(None));

type DmaUart = bsp::hal::dma::Peripheral<
    bsp::hal::uart::UART<bsp::hal::iomuxc::consts::U2>,
    u8,
    TxBuffer,
    RxBuffer,
>;

/// A DMA peripheral adapter around the UART2 peripheral. It sends bytes.
static mut DMA_PERIPHERAL: Option<DmaUart> = None;

/// Flags for the main loop
static TX_READY: AtomicBool = AtomicBool::new(false);
static RX_READY: AtomicBool = AtomicBool::new(false);

/// The DMA interrupt matches the selected DMA channels in the demo's setup.
#[cortex_m_rt::interrupt]
unsafe fn DMA7_DMA23() {
    let uart = DMA_PERIPHERAL.as_mut().unwrap();

    // Safe to create a critical section. This won't be preempted by a higher-priority
    // exception.
    let cs = cortex_m::interrupt::CriticalSection::new();

    if uart.is_transfer_interrupt() {
        uart.transfer_clear_interrupt();
        let mut tx_buffer = TX_BUFFER.borrow(&cs).borrow_mut();
        *tx_buffer = uart.transfer_complete();
        TX_READY.store(true, Ordering::Release);
    }
    if uart.is_receive_interrupt() {
        uart.receive_clear_interrupt();
        let mut rx_buffer = RX_BUFFER.borrow(&cs).borrow_mut();
        *rx_buffer = uart.receive_complete();
        RX_READY.store(true, Ordering::Release);
    }
}

#[entry]
fn main() -> ! {
    let mut peripherals = bsp::Peripherals::take().unwrap();
    let mut systick = bsp::SysTick::new(cortex_m::Peripherals::take().unwrap().SYST);
    usb_io::init().unwrap();
    let pins = bsp::t40::into_pins(peripherals.iomuxc);

    systick.delay(5_000);
    let uarts = peripherals.uart.clock(
        &mut peripherals.ccm.handle,
        bsp::hal::ccm::uart::ClockSelect::OSC,
        bsp::hal::ccm::uart::PrescalarSelect::DIVIDE_1,
    );
    let uart = uarts.uart2.init(pins.p14, pins.p15, BAUD).unwrap();

    let mut dma_channels = peripherals.dma.clock(&mut peripherals.ccm.handle);
    let mut tx_channel = dma_channels[7].take().unwrap();
    let mut rx_channel = dma_channels[23].take().unwrap();

    tx_channel.set_interrupt_on_completion(true);
    rx_channel.set_interrupt_on_completion(true);

    let dma_uart = unsafe {
        DMA_PERIPHERAL = Some(bsp::hal::dma::Peripheral::new_bidirectional(
            uart, tx_channel, rx_channel,
        ));
        cortex_m::peripheral::NVIC::unmask(interrupt::DMA7_DMA23);
        DMA_PERIPHERAL.as_mut().unwrap()
    };
    let mut led = bsp::configure_led(pins.p13);

    let rx_buffer = match bsp::hal::dma::Circular::new(&RX_MEM.0) {
        Ok(circular) => circular,
        Err(error) => {
            log::error!("Unable to create circular RX buffer: {:?}", error);
            panic!();
        }
    };

    free(|cs| {
        *RX_BUFFER.borrow(cs).borrow_mut() = Some(rx_buffer);
        *TX_BUFFER.borrow(cs).borrow_mut() = bsp::hal::dma::Linear::new(&TX_MEM);
    });

    'start: loop {
        RX_READY.store(false, Ordering::Release);
        TX_READY.store(false, Ordering::Release);

        let mut rx_buffer = match free(|cs| RX_BUFFER.borrow(cs).borrow_mut().take()) {
            None => {
                log::error!("No receive buffer!");
                panic!();
            }
            Some(rx_buffer) => rx_buffer,
        };
        rx_buffer.reserve(1);

        // Schedule an initial receive
        let res = dma_uart.start_receive(rx_buffer);
        if let Err(err) = res {
            log::error!("Error scheduling DMA receive: {:?}", err);
            panic!();
        }

        loop {
            cortex_m::asm::wfi();
            if RX_READY.load(Ordering::Acquire) {
                led.toggle();
                RX_READY.store(false, Ordering::Release);
                let mut rx_buffer = free(|cs| RX_BUFFER.borrow(cs).borrow_mut().take()).unwrap();
                let value = match rx_buffer.pop() {
                    Some(v) => v,
                    None => {
                        log::warn!("Nothing to pop! Returning '0'");
                        0
                    }
                };
                free(|cs| *RX_BUFFER.borrow(cs).borrow_mut() = Some(rx_buffer));
                log::info!("Received: {}", value);
                let mut tx_buffer = match free(|cs| TX_BUFFER.borrow(cs).borrow_mut().take()) {
                    None => {
                        log::error!("No transfer buffer!");
                        panic!();
                    }
                    Some(tx_buffer) => tx_buffer,
                };

                tx_buffer.as_mut_elements()[0] = value;
                tx_buffer.set_transfer_len(1);
                let res = dma_uart.start_transfer(tx_buffer);
                if let Err(err) = res {
                    log::warn!("Error scheduling DMA transfer: {:?}", err);
                    panic!();
                }
            } else if TX_READY.load(Ordering::Acquire) {
                continue 'start;
            }
        }
    }
}
