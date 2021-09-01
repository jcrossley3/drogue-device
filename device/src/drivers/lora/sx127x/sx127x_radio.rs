use crate::traits::lora::LoraError as DriverError;
use embedded_hal::blocking::spi::{Transfer, Write};
use embedded_hal::digital::v2::OutputPin;
use heapless::Vec;
use lorawan_device::{
    radio::{
        Bandwidth, Error as LoraError, Event as LoraEvent, PhyRxTx, PhyRxTxBuf,
        Response as LoraResponse, RxQuality, SpreadingFactor,
    },
    Timings,
};

use super::sx127x_lora::{LoRa, RadioMode, IRQ};

pub struct Sx127xRadio<SPI, CS, RESET, E>
where
    SPI: Transfer<u8, Error = E> + Write<u8, Error = E>,
    CS: OutputPin,
    RESET: OutputPin,
{
    radio: LoRa<SPI, CS, RESET>,
    radio_state: State,
    buffer: RadioBuffer,
}

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum State {
    Idle,
    Txing,
    Rxing,
}

fn spreading_factor_to_u8(sf: SpreadingFactor) -> u8 {
    match sf {
        SpreadingFactor::_7 => 7,
        SpreadingFactor::_8 => 8,
        SpreadingFactor::_9 => 9,
        SpreadingFactor::_10 => 10,
        SpreadingFactor::_11 => 11,
        SpreadingFactor::_12 => 12,
    }
}

fn bandwidth_to_i64(bw: Bandwidth) -> i64 {
    match bw {
        Bandwidth::_125KHz => 125_000,
        Bandwidth::_250KHz => 250_000,
        Bandwidth::_500KHz => 500_000,
    }
}

impl<SPI, CS, RESET, E> Sx127xRadio<SPI, CS, RESET, E>
where
    SPI: Transfer<u8, Error = E> + Write<u8, Error = E>,
    CS: OutputPin,
    RESET: OutputPin,
{
    pub fn new(spi: SPI, cs: CS, reset: RESET) -> Self {
        Self {
            radio_state: State::Idle,
            radio: LoRa::new(spi, cs, reset),
            buffer: RadioBuffer { packet: Vec::new() },
        }
    }

    pub async fn reset(&mut self) -> Result<(), DriverError> {
        self.radio
            .reset()
            .await
            .map_err(|_| DriverError::OtherError)
    }

    pub fn handle_event_idle(
        &mut self,
        event: LoraEvent<Self>,
    ) -> (State, Result<LoraResponse<Self>, LoraError<Self>>) {
        match event {
            LoraEvent::TxRequest(config, buf) => {
                //trace!("Set config: {:?}", config);
                let result = (move || {
                    self.radio.set_tx_power(14, 0)?;
                    self.radio.set_frequency(config.rf.frequency)?;
                    // TODO: Modify radio to support other coding rates
                    self.radio.set_coding_rate_4(5)?;
                    self.radio
                        .set_signal_bandwidth(bandwidth_to_i64(config.rf.bandwidth))?;
                    self.radio
                        .set_spreading_factor(spreading_factor_to_u8(config.rf.spreading_factor))?;

                    self.radio.set_preamble_length(8)?;
                    self.radio.set_lora_pa_ramp()?;
                    self.radio.set_lora_sync_word()?;
                    self.radio.set_invert_iq(false)?;
                    self.radio.set_crc(true)?;

                    let len = buf.packet.len();
                    self.radio.set_dio0_tx_done()?;
                    self.radio.transmit_payload(&buf.packet[..len])
                })();
                match result {
                    Ok(_) => (State::Txing, Ok(LoraResponse::Txing)),
                    Err(_) => (State::Idle, Err(LoraError::PhyError(()))),
                }
            }
            LoraEvent::RxRequest(config) => {
                // trace!("Set RX config: {:?}", config);
                let result = (move || {
                    self.radio.reset_payload_length()?;
                    self.radio.set_frequency(config.frequency)?;
                    // TODO: Modify radio to support other coding rates
                    self.radio.set_coding_rate_4(5)?;
                    self.radio
                        .set_signal_bandwidth(bandwidth_to_i64(config.bandwidth))?;
                    self.radio
                        .set_spreading_factor(spreading_factor_to_u8(config.spreading_factor))?;

                    self.radio.set_preamble_length(8)?;
                    self.radio.set_lora_sync_word()?;
                    self.radio.set_invert_iq(true)?;
                    self.radio.set_crc(true)?;

                    self.radio.set_dio0_rx_done()?;
                    self.radio.set_mode(RadioMode::RxContinuous)

                    /*
                    let irq_flags = self.radio.irq_flags().ok().unwrap();
                    let irq_flags_mask = self.radio.irq_flags_mask().ok().unwrap();
                    info!(
                        "RX STARTED, IRQ Flags: 0x{:x}, Mask: 0x{:x}",
                        irq_flags,
                        irq_flags_mask
                    );
                    r*/
                })();
                match result {
                    Ok(_) => (State::Rxing, Ok(LoraResponse::Rxing)),
                    Err(_) => (State::Rxing, Err(LoraError::PhyError(()))),
                }
            }
            // deny any events while idle; they are unexpected
            LoraEvent::PhyEvent(_) => (State::Idle, Err(LoraError::PhyError(()))),
            LoraEvent::CancelRx => (State::Idle, Err(LoraError::CancelRxWhileIdle)),
        }
    }

    pub fn handle_event_txing(
        &mut self,
        event: LoraEvent<Self>,
    ) -> (State, Result<LoraResponse<Self>, LoraError<Self>>) {
        match event {
            LoraEvent::PhyEvent(phyevent) => match phyevent {
                RadioPhyEvent::Irq => {
                    self.radio.set_mode(RadioMode::Stdby).ok().unwrap();
                    let irq = self.radio.clear_irq().ok().unwrap();
                    if (irq & IRQ::IrqTxDoneMask.addr()) != 0 {
                        (State::Idle, Ok(LoraResponse::TxDone(0)))
                    } else {
                        (State::Txing, Ok(LoraResponse::Txing))
                    }
                }
            },
            LoraEvent::TxRequest(_, _) => (State::Txing, Err(LoraError::TxRequestDuringTx)),
            LoraEvent::RxRequest(_) => (State::Txing, Err(LoraError::RxRequestDuringTx)),
            LoraEvent::CancelRx => (State::Txing, Err(LoraError::CancelRxDuringTx)),
        }
    }

    pub fn handle_event_rxing(
        &mut self,
        event: LoraEvent<Self>,
    ) -> (State, Result<LoraResponse<Self>, LoraError<Self>>) {
        match event {
            LoraEvent::PhyEvent(phyevent) => match phyevent {
                RadioPhyEvent::Irq => {
                    self.radio.set_mode(RadioMode::Stdby).ok().unwrap();
                    let irq = self.radio.clear_irq().ok().unwrap();
                    if (irq & IRQ::IrqRxDoneMask.addr()) != 0 {
                        let rssi = self.radio.get_packet_rssi().unwrap_or(0) as i16;
                        let snr = self.radio.get_packet_snr().unwrap_or(0.0) as i8;
                        if let Ok(size) = self.radio.read_packet_size() {
                            if let Ok(packet) = self.radio.read_packet() {
                                self.buffer.packet.clear();
                                self.buffer
                                    .packet
                                    .extend_from_slice(&packet[..size])
                                    .ok()
                                    .unwrap();
                            }
                        }
                        self.radio.set_mode(RadioMode::Sleep).ok().unwrap();
                        (
                            State::Idle,
                            Ok(LoraResponse::RxDone(RxQuality::new(rssi, snr))),
                        )
                    } else {
                        (State::Rxing, Ok(LoraResponse::Rxing))
                    }
                }
            },
            LoraEvent::CancelRx => {
                self.radio.set_mode(RadioMode::Sleep).ok().unwrap();
                (State::Idle, Ok(LoraResponse::Idle))
            }
            LoraEvent::TxRequest(_, _) => (State::Rxing, Err(LoraError::TxRequestDuringTx)),
            LoraEvent::RxRequest(_) => (State::Rxing, Err(LoraError::RxRequestDuringRx)),
        }
    }
}

impl<SPI, CS, RESET, E> Timings for Sx127xRadio<SPI, CS, RESET, E>
where
    SPI: Transfer<u8, Error = E> + Write<u8, Error = E>,
    CS: OutputPin,
    RESET: OutputPin,
{
    fn get_rx_window_offset_ms(&self) -> i32 {
        -500
    }
    fn get_rx_window_duration_ms(&self) -> u32 {
        800
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RadioPhyEvent {
    Irq,
}

pub struct RadioBuffer {
    pub packet: Vec<u8, 256>,
}

impl PhyRxTxBuf for RadioBuffer {
    fn clear_buf(&mut self) {
        self.packet.clear();
    }

    fn extend_buf(&mut self, buf: &[u8]) {
        self.packet.extend_from_slice(buf).unwrap();
    }
}

impl Default for RadioBuffer {
    fn default() -> Self {
        Self { packet: Vec::new() }
    }
}

impl AsMut<[u8]> for RadioBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        self.packet.as_mut()
    }
}

impl AsRef<[u8]> for RadioBuffer {
    fn as_ref(&self) -> &[u8] {
        self.packet.as_ref()
    }
}

impl<SPI, CS, RESET, E> PhyRxTx for Sx127xRadio<SPI, CS, RESET, E>
where
    SPI: Transfer<u8, Error = E> + Write<u8, Error = E>,
    CS: OutputPin,
    RESET: OutputPin,
{
    type PhyBuf = RadioBuffer;
    type PhyEvent = RadioPhyEvent;
    type PhyError = ();
    type PhyResponse = ();

    fn get_mut_radio(&mut self) -> &mut Self {
        self
    }

    fn get_received_packet(&mut self) -> &mut Self::PhyBuf {
        &mut self.buffer
    }

    fn handle_event(
        &mut self,
        event: LoraEvent<Self>,
    ) -> Result<LoraResponse<Self>, LoraError<Self>>
    where
        Self: core::marker::Sized,
    {
        let (new_state, response) = match &self.radio_state {
            State::Idle => self.handle_event_idle(event),
            State::Txing => self.handle_event_txing(event),
            State::Rxing => self.handle_event_rxing(event),
        };
        self.radio_state = new_state;
        response
    }
}
