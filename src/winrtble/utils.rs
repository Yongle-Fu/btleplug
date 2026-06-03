// btleplug Source Code File
//
// Copyright 2020 Nonpolynomial Labs LLC. All rights reserved.
//
// Licensed under the BSD 3-Clause license. See LICENSE file in the project root
// for full license information.
//
// Some portions of this file are taken and/or modified from Rumble
// (https://github.com/mwylde/rumble), using a dual MIT/Apache License under the
// following copyright:
//
// Copyright (c) 2014 The Rust Project Developers

use crate::{Error, Result, api::CharPropFlags};
use std::str::FromStr;
use uuid::Uuid;
use windows::core::GUID;
use windows::{
    Devices::Bluetooth::{
        BluetoothLEDevice,
        GenericAttributeProfile::{
            GattCharacteristic, GattCharacteristicProperties,
            GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
            GattDescriptor,
        },
    },
    Storage::Streams::{DataReader, IBuffer},
};

pub fn is_encryption_error(err: &windows::core::Error) -> bool {
    let code = err.code().0;
    // 0x8065000F: E_BLUETOOTH_ATT_INSUFFICIENT_ENCRYPTION
    // 0x80650009: E_BLUETOOTH_ATT_INSUFFICIENT_AUTHENTICATION
    if code == 0x8065000F_u32 as i32 || code == 0x80650009_u32 as i32 {
        return true;
    }
    let msg = err.to_string().to_lowercase();
    msg.contains("encryption") || msg.contains("insufficient")
}

pub async fn pair_device(device: &BluetoothLEDevice) -> Result<()> {
    let dev_info = device
        .DeviceInformation()
        .map_err(|e| Error::Other(format!("Failed to get DeviceInformation: {:?}", e).into()))?;
    let pairing = dev_info
        .Pairing()
        .map_err(|e| Error::Other(format!("Failed to get DeviceInformationPairing: {:?}", e).into()))?;

    if !pairing.IsPaired().unwrap_or(false) {
        log::info!("Device is not paired. Initiating pairing...");
        let op = pairing
            .PairAsync()
            .map_err(|e| Error::Other(format!("PairAsync call failed: {:?}", e).into()))?;
        let result = op
            .await
            .map_err(|e| Error::Other(format!("PairAsync operation failed: {:?}", e).into()))?;
        let status = result
            .Status()
            .map_err(|e| Error::Other(format!("Failed to get PairingStatus: {:?}", e).into()))?;
        log::info!("Pairing completed with status: {:?}", status);
        // 0 = Paired, 3 = AlreadyPaired
        if status.0 != 0 && status.0 != 3 {
            return Err(Error::Other(format!("Pairing failed: {:?}", status).into()));
        }
        // Give Windows a moment to stabilize the encryption state
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    } else {
        log::info!("Device is already paired.");
    }
    Ok(())
}

pub async fn pair_from_characteristic(
    characteristic: &GattCharacteristic,
) -> Result<()> {
    let service = characteristic
        .Service()
        .map_err(|e| Error::Other(format!("Failed to get Service: {:?}", e).into()))?;
    let device = service
        .Device()
        .map_err(|e| Error::Other(format!("Failed to get Device: {:?}", e).into()))?;
    pair_device(&device).await
}

pub async fn pair_from_descriptor(
    descriptor: &GattDescriptor,
) -> Result<()> {
    let characteristic = descriptor
        .Characteristic()
        .map_err(|e| Error::Other(format!("Failed to get Characteristic: {:?}", e).into()))?;
    pair_from_characteristic(&characteristic).await
}

pub fn to_error(status: GattCommunicationStatus) -> Result<()> {
    if status == GattCommunicationStatus::AccessDenied {
        Err(Error::PermissionDenied)
    } else if status == GattCommunicationStatus::Unreachable {
        Err(Error::NotConnected)
    } else if status == GattCommunicationStatus::Success {
        Ok(())
    } else if status == GattCommunicationStatus::ProtocolError {
        Err(Error::NotSupported("ProtocolError".to_string()))
    } else {
        Err(Error::Other("Communication Error:".to_string().into()))
    }
}

pub fn to_descriptor_value(
    properties: GattCharacteristicProperties,
) -> GattClientCharacteristicConfigurationDescriptorValue {
    let notify = GattCharacteristicProperties::Notify;
    let indicate = GattCharacteristicProperties::Indicate;
    if properties & indicate == indicate {
        GattClientCharacteristicConfigurationDescriptorValue::Indicate
    } else if properties & notify == notify {
        GattClientCharacteristicConfigurationDescriptorValue::Notify
    } else {
        GattClientCharacteristicConfigurationDescriptorValue::None
    }
}

pub fn to_uuid(uuid: &GUID) -> Uuid {
    let guid_s = format!("{:?}", uuid);
    Uuid::from_str(&guid_s).unwrap()
}

pub fn to_vec(buffer: &IBuffer) -> Vec<u8> {
    let reader = DataReader::FromBuffer(buffer).unwrap();
    let len = reader.UnconsumedBufferLength().unwrap() as usize;
    let mut data = vec![0u8; len];
    reader.ReadBytes(&mut data).unwrap();
    data
}

#[allow(dead_code)]
pub fn to_guid(uuid: &Uuid) -> GUID {
    let (data1, data2, data3, data4) = uuid.as_fields();
    GUID::from_values(data1, data2, data3, data4.to_owned())
}

pub fn to_char_props(props: &GattCharacteristicProperties) -> CharPropFlags {
    let mut flags = CharPropFlags::default();
    if *props & GattCharacteristicProperties::Broadcast != GattCharacteristicProperties::None {
        flags |= CharPropFlags::BROADCAST;
    }
    if *props & GattCharacteristicProperties::Read != GattCharacteristicProperties::None {
        flags |= CharPropFlags::READ;
    }
    if *props & GattCharacteristicProperties::WriteWithoutResponse
        != GattCharacteristicProperties::None
    {
        flags |= CharPropFlags::WRITE_WITHOUT_RESPONSE;
    }
    if *props & GattCharacteristicProperties::Write != GattCharacteristicProperties::None {
        flags |= CharPropFlags::WRITE;
    }
    if *props & GattCharacteristicProperties::Notify != GattCharacteristicProperties::None {
        flags |= CharPropFlags::NOTIFY;
    }
    if *props & GattCharacteristicProperties::Indicate != GattCharacteristicProperties::None {
        flags |= CharPropFlags::INDICATE;
    }
    if *props & GattCharacteristicProperties::AuthenticatedSignedWrites
        != GattCharacteristicProperties::None
    {
        flags |= CharPropFlags::AUTHENTICATED_SIGNED_WRITES;
    }
    if *props & GattCharacteristicProperties::ExtendedProperties
        != GattCharacteristicProperties::None
    {
        flags |= CharPropFlags::EXTENDED_PROPERTIES;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_uuid_to_guid_conversion() {
        let uuid_str = "10B201FF-5B3B-45A1-9508-CF3EFCD7BBAF";
        let uuid = Uuid::from_str(uuid_str).unwrap();

        let guid_converted = to_guid(&uuid);

        let guid_expected = GUID::try_from(uuid_str).unwrap();
        assert_eq!(guid_converted, guid_expected);
    }

    #[test]
    fn check_guid_to_uuid_conversion() {
        let uuid_str = "10B201FF-5B3B-45A1-9508-CF3EFCD7BBAF";
        let guid = GUID::try_from(uuid_str).unwrap();

        let uuid_converted = to_uuid(&guid);

        let uuid_expected = Uuid::from_str(uuid_str).unwrap();
        assert_eq!(uuid_converted, uuid_expected);
    }
}
