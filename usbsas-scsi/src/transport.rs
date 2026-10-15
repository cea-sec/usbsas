//! Abstraction over the USB device handle, so the protocol logic can be
//! exercised without hardware.

use rusb::{DeviceHandle, UsbContext};
use std::time::Duration;

/// The USB operations needed to drive a Bulk-Only mass storage device.
///
/// Implemented for [`rusb::DeviceHandle`]; method semantics follow rusb's.
pub trait UsbTransport {
    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize>;
    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> rusb::Result<usize>;
    fn read_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
        timeout: Duration,
    ) -> rusb::Result<usize>;
    fn write_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        buf: &[u8],
        timeout: Duration,
    ) -> rusb::Result<usize>;
    fn clear_halt(&self, endpoint: u8) -> rusb::Result<()>;
    fn set_alternate_setting(&self, interface: u8, setting: u8) -> rusb::Result<()>;
    fn reset(&self) -> rusb::Result<()>;
    /// `wMaxPacketSize` of `endpoint` in the active configuration, if found.
    fn max_packet_size(&self, endpoint: u8) -> Option<u16>;
}

impl<T: UsbContext> UsbTransport for DeviceHandle<T> {
    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize> {
        DeviceHandle::read_bulk(self, endpoint, buf, timeout)
    }

    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> rusb::Result<usize> {
        DeviceHandle::write_bulk(self, endpoint, buf, timeout)
    }

    fn read_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        buf: &mut [u8],
        timeout: Duration,
    ) -> rusb::Result<usize> {
        DeviceHandle::read_control(self, request_type, request, value, index, buf, timeout)
    }

    fn write_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        buf: &[u8],
        timeout: Duration,
    ) -> rusb::Result<usize> {
        DeviceHandle::write_control(self, request_type, request, value, index, buf, timeout)
    }

    fn clear_halt(&self, endpoint: u8) -> rusb::Result<()> {
        DeviceHandle::clear_halt(self, endpoint)
    }

    fn set_alternate_setting(&self, interface: u8, setting: u8) -> rusb::Result<()> {
        DeviceHandle::set_alternate_setting(self, interface, setting)
    }

    fn reset(&self) -> rusb::Result<()> {
        DeviceHandle::reset(self)
    }

    fn max_packet_size(&self, endpoint: u8) -> Option<u16> {
        let config = self.device().active_config_descriptor().ok()?;
        config
            .interfaces()
            .flat_map(|interface| interface.descriptors())
            .flat_map(|desc| desc.endpoint_descriptors().collect::<Vec<_>>())
            .find(|ep| ep.address() == endpoint)
            // Bits 10..12 encode high-bandwidth multipliers, not the packet size.
            .map(|ep| ep.max_packet_size() & 0x07ff)
    }
}
