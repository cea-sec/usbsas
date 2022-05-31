//! SCSI implementation

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use log::{debug, error};
use rusb::{request_type, DeviceHandle, Direction, Recipient, RequestType, UsbContext};
use std::{
    convert::TryFrom,
    io::{self, ErrorKind},
    time::Duration,
    {thread, time},
};

const LIBUSB_ENDPOINT_IN: u8 = 0x80;
const LIBUSB_ENDPOINT_OUT: u8 = 0x00;

const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_REQUEST_SENSE: u8 = 0x3;
const SCSI_MAX_READ_SECTORS: u64 = 0x800;

//#[derive(Debug, Default)]
pub struct ScsiUsb<T: UsbContext> {
    pub handle: DeviceHandle<T>,
    pub tag: u32,
    pub lun: Option<u8>,
    pub endpoint_in: u8,
    pub endpoint_out: u8,
    pub timeout: Duration,
}

impl<T: UsbContext> ScsiUsb<T> {
    pub fn new(
        handle: DeviceHandle<T>,
        interface_num: u8,
        interface_alt: u8,
        endpoint_in: u8,
        endpoint_out: u8,
        timeout: Duration,
    ) -> ScsiUsb<T> {
        let mut scsi = ScsiUsb {
            handle,
            tag: 1,
            lun: None,
            endpoint_in,
            endpoint_out,
            timeout,
        };
        scsi.set_active_interface(interface_num, interface_alt);
        scsi
    }

    fn command_to_size(&self, cmd: u8) -> Option<u8> {
        match cmd {
            0x00..=0x1F => Some(6),
            0x20..=0x5F => Some(10),
            0x60..=0x7F => None,
            0x80..=0x9F => Some(16),
            0xA0..=0xBF => Some(12),
            0xC0..=0xFF => None,
        }
    }

    fn cbw_init(
        &self,
        buffer: &[u8],
        dir: u8,
        command_data: [u8; 16],
    ) -> Result<[u8; 31], io::Error> {
        let mut cbw: [u8; 31] = [0; 31];
        cbw[0] = 0x55;
        cbw[1] = 0x53;
        cbw[2] = 0x42;
        cbw[3] = 0x43;

        LittleEndian::write_u32(&mut cbw[4..8], self.tag);
        LittleEndian::write_u32(&mut cbw[8..12], buffer.len() as u32);

        cbw[12] = dir;
        cbw[13] = self
            .lun
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "lun must be set"))?;
        cbw[14] = self
            .command_to_size(command_data[0])
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "cbw error"))?;
        cbw[15..31].copy_from_slice(&command_data);
        Ok(cbw)
    }

    fn ack_data(&mut self) -> Result<u8, io::Error> {
        let mut csw: [u8; 13] = [0; 13];
        if self
            .handle
            .read_bulk(self.endpoint_in, &mut csw, self.timeout)
            .is_err()
        {
            return Err(io::Error::new(ErrorKind::Other, "Usb ack error"));
        }
        self.tag += 1;
        Ok(csw[12])
    }

    fn read_bulk(&mut self, data: &mut [u8]) -> Result<usize, io::Error> {
        match self.handle.read_bulk(self.endpoint_in, data, self.timeout) {
            Ok(size) => {
                if size != data.len() {
                    Err(io::Error::new(ErrorKind::Other, "Usb read_bulk size error"))
                } else {
                    Ok(size)
                }
            }
            Err(err) => Err(io::Error::new(
                ErrorKind::Other,
                format!("Usb read_bulk error: {}", err),
            )),
        }
    }

    fn write_bulk(&mut self, data: &[u8]) -> Result<usize, io::Error> {
        match self
            .handle
            .write_bulk(self.endpoint_out, data, self.timeout)
        {
            Ok(size) => {
                if size != data.len() {
                    Err(io::Error::new(
                        ErrorKind::Other,
                        "Usb write_bulk size error",
                    ))
                } else {
                    Ok(size)
                }
            }
            Err(err) => Err(io::Error::new(
                ErrorKind::Other,
                format!("Usb write_bulk error: {}", err),
            )),
        }
    }

    fn bulk_transfer_read(
        &mut self,
        command_data: [u8; 16],
        buffer: &mut [u8],
    ) -> Result<u8, io::Error> {
        let cbw = self.cbw_init(buffer, LIBUSB_ENDPOINT_IN, command_data)?;
        self.write_bulk(&cbw)?;
        if !buffer.is_empty() {
            self.read_bulk(buffer)?;
        }
        self.ack_data()
    }

    fn bulk_transfer_write(
        &mut self,
        command_data: [u8; 16],
        buffer: &mut [u8],
    ) -> Result<u8, io::Error> {
        let cbw = self.cbw_init(buffer, LIBUSB_ENDPOINT_OUT, command_data)?;
        self.write_bulk(&cbw)?;
        self.write_bulk(buffer)?;
        self.ack_data()
    }

    fn scsi_test_unit_ready(&mut self, buffer: &mut [u8]) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_TEST_UNIT_READY;
        let cbw = self.cbw_init(buffer, LIBUSB_ENDPOINT_OUT, command_data)?;
        self.write_bulk(&cbw)?;
        self.ack_data()
    }

    fn scsi_read_capacity_10(&mut self, buffer: &mut [u8]) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_READ_CAPACITY_10;
        self.bulk_transfer_read(command_data, buffer)
    }

    pub fn scsi_read_10(
        &mut self,
        buffer: &mut [u8],
        offset: u64,
        count: u64,
    ) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_READ_10;
        BigEndian::write_u32(
            &mut command_data[2..6],
            u32::try_from(offset).map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "Couldn't convert u64 to u32")
            })?,
        );
        BigEndian::write_u16(
            &mut command_data[7..9],
            u16::try_from(count).map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "Couldn't convert u64 to u16")
            })?,
        );
        self.bulk_transfer_read(command_data, buffer)
    }

    pub fn scsi_write_10(
        &mut self,
        buffer: &mut [u8],
        offset: u64,
        count: u64,
    ) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_WRITE_10;
        BigEndian::write_u32(
            &mut command_data[2..6],
            u32::try_from(offset).map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "Couldn't convert u64 to u32")
            })?,
        );
        BigEndian::write_u16(
            &mut command_data[7..9],
            u16::try_from(count).map_err(|_| {
                io::Error::new(ErrorKind::InvalidData, "Couldn't convert u64 to u16")
            })?,
        );
        self.bulk_transfer_write(command_data, buffer)
    }

    fn scsi_inquiry(&mut self, buffer: &mut [u8]) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_INQUIRY;
        BigEndian::write_u32(&mut command_data[1..5], 0x24);
        self.bulk_transfer_read(command_data, buffer)
    }

    fn scsi_request_sense(&mut self, buffer: &mut [u8]) -> Result<u8, io::Error> {
        let mut command_data: [u8; 16] = [0; 16];
        command_data[0] = SCSI_REQUEST_SENSE;
        BigEndian::write_u32(&mut command_data[1..5], buffer.len() as u32);
        self.bulk_transfer_read(command_data, buffer)
    }

    fn get_max_lun(&mut self) -> u8 {
        let mut buffer: [u8; 1] = [0; 1];
        let _len = self.handle.read_control(
            request_type(Direction::In, RequestType::Class, Recipient::Interface),
            0xfe, // Get max lun
            0,
            0,
            &mut buffer,
            self.timeout,
        );
        buffer[0]
    }

    pub fn set_active_conf(&mut self) {
        let buffer: [u8; 0] = [0; 0];
        let _len = self.handle.write_control(
            request_type(Direction::Out, RequestType::Standard, Recipient::Device),
            0x9, // set active configuration
            1,
            0,
            &buffer,
            self.timeout,
        );
    }

    fn set_active_interface(&mut self, interface_num: u8, interface_alt: u8) {
        let buffer: [u8; 0] = [0; 0];
        let _len = self.handle.write_control(
            request_type(Direction::Out, RequestType::Standard, Recipient::Interface),
            11, // set active interface
            interface_num as u16,
            interface_alt as u16,
            &buffer,
            self.timeout,
        );
    }

    pub fn read_sectors(
        &mut self,
        offset: u64,
        count: u64,
        block_size: usize,
    ) -> Result<Vec<u8>, io::Error> {
        let mut buffer = Vec::new();
        let mut offset = offset;
        let mut remaining_sectors = count;
        while remaining_sectors != 0 {
            let sectors_to_read = std::cmp::min(remaining_sectors, SCSI_MAX_READ_SECTORS);
            let mut tmp = vec![0; block_size * sectors_to_read as usize];
            self.scsi_read_10(&mut tmp, offset, sectors_to_read)?;
            offset += sectors_to_read;
            buffer.append(&mut tmp);
            remaining_sectors -= sectors_to_read;
        }
        Ok(buffer)
    }

    pub fn init_mass_storage(&mut self) -> Result<(u32, u32, u64), io::Error> {
        let max_lun = self.get_max_lun();
        debug!("init mass storage. Luns: {}", max_lun);
        // Store luns which Direct access device set
        let mut lun_dad = vec![];
        for lun in 0..=max_lun {
            self.lun = Some(lun);
            let mut buffer: [u8; 36] = [0; 36];
            self.scsi_inquiry(&mut buffer)?;
            let lun_type = buffer[0] & 0x1f;
            debug!("Lun {} of type {}", lun, lun_type);
            if lun_type == 0 {
                // Device is Direct access decice
                lun_dad.push(lun);
            }
        }

        debug!("Direct access devices luns: {:?}", lun_dad);

        /* For each lun, test if ready or not present  */

        let mut is_ok = false;
        for lun in lun_dad.iter() {
            debug!("Test lun {}", lun);
            self.lun = Some(*lun);
            let mut buffer: [u8; 36] = [0; 36];
            self.scsi_inquiry(&mut buffer)?;
            for _ in 0..100 {
                debug!("Test unit ready...");
                let mut buffer: [u8; 0] = [0; 0];
                match self.scsi_test_unit_ready(&mut buffer) {
                    Ok(0) => {
                        /* Everything is ok */
                        is_ok = true;
                        break;
                    }
                    Ok(ret) => {
                        debug!("Test unit response {}", ret);
                        debug!("Test unit buffer {:?}", buffer);

                        let mut buffer: [u8; 18] = [0; 18];
                        /*
                         * https://www.seagate.com/files/staticfiles/support/docs/manual/Interface%20manuals/100293068j.pdf
                         * 2.4.1.2 Fixed format sense data
                         */
                        match self.scsi_request_sense(&mut buffer) {
                            Ok(ret) => {
                                debug!("Request Sense {}", ret);
                                debug!("Request Sense buffer {:?}", buffer);
                                if buffer[0] & 0x70 == 0x70 {
                                    /* Sense response error code */
                                    match buffer[2] & 0xF {
                                        0x0 => {
                                            debug!("No sense");
                                        }
                                        1 => {
                                            debug!("Recovered error");
                                        }
                                        0x2 => {
                                            /* Unit not ready */
                                            /* 2.4.1.6 Additional Sense and Additional Sense Qualifier codes */
                                            match buffer[12] {
                                                0x4 => {
                                                    /* Logical Unit Not Ready, Cause Not Reportable*/

                                                    /* Wait 200ms */
                                                    let ten_millis =
                                                        time::Duration::from_millis(200);
                                                    thread::sleep(ten_millis);
                                                    continue;
                                                }
                                                _ => {
                                                    /* XXX TODO: All others code signal a fail? */
                                                    error!(
                                                        "{}",
                                                        &format!(
                                                            "Sense sub error code: {:?}",
                                                            &buffer[12..13]
                                                        )
                                                    );
                                                    is_ok = false;
                                                    break;
                                                }
                                            }
                                        }
                                        3 | 4 | 5 => {
                                            error!("Medium error");
                                            is_ok = false;
                                            break;
                                        }
                                        0x6 => {
                                            debug!(
                                                "Unit attention: ASC: 0x{:x} ASCQ: 0x{:x}",
                                                buffer[12], buffer[13]
                                            );
                                            /* Wait 200ms */
                                            let ten_millis = time::Duration::from_millis(200);
                                            thread::sleep(ten_millis);
                                            continue;
                                        }
                                        0xF => {
                                            debug!("Sense 0xF ok");
                                        }
                                        _ => {
                                            error!("Unhandled sense code: {:?}", buffer[2]);
                                        }
                                    }
                                } else {
                                    error!("Strange error code");
                                }
                            }
                            Err(_) => {
                                error!("Error during request sense");
                                is_ok = false;
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        return Err(io::Error::new(ErrorKind::Other, "Test usb key fail"));
                    }
                }
                if is_ok {
                    break;
                }
            }
        }

        if !is_ok {
            error!("No lun found!");
            return Err(io::Error::new(ErrorKind::Other, "Cannot find lun"));
        }

        let mut buffer: [u8; 8] = [0; 8];
        match self.scsi_read_capacity_10(&mut buffer) {
            Ok(_) => {}
            Err(_) => {
                return Err(io::Error::new(ErrorKind::Other, "Cannot read capacity"));
            }
        }

        assert!(buffer[4] == 0 && buffer[5] == 0);

        let max_lba: u32 = BigEndian::read_u32(&buffer[0..4]);
        let block_size: u32 = BigEndian::read_u32(&buffer[4..8]);
        let dev_size: u64 = (u64::from(max_lba) + 1) * (u64::from(block_size));

        Ok((max_lba, block_size, dev_size))
    }
}
