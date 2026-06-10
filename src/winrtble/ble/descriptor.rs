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

use super::super::utils;
use crate::{Error, Result, api::Descriptor};
use std::future::IntoFuture;
use uuid::Uuid;
use windows::{
    Devices::Bluetooth::{
        BluetoothCacheMode,
        GenericAttributeProfile::{GattCommunicationStatus, GattDescriptor, GattCharacteristic},
    },
    Storage::Streams::{DataReader, DataWriter},
};

#[derive(Debug)]
pub struct BLEDescriptor {
    characteristic: GattCharacteristic,
    descriptor: GattDescriptor,
}

impl BLEDescriptor {
    pub fn new(characteristic: GattCharacteristic, descriptor: GattDescriptor) -> Self {
        Self { characteristic, descriptor }
    }

    pub fn uuid(&self) -> Uuid {
        utils::to_uuid(&self.descriptor.Uuid().unwrap())
    }

    pub fn to_descriptor(&self, service_uuid: Uuid, characteristic_uuid: Uuid) -> Descriptor {
        let uuid = self.uuid();
        Descriptor {
            uuid,
            service_uuid,
            characteristic_uuid,
        }
    }

    pub async fn write_value(&self, data: &[u8]) -> Result<()> {
        let mut attempts = 0;
        loop {
            let writer = DataWriter::new()?;
            writer.WriteBytes(data)?;
            let buffer = writer.DetachBuffer()?;
            let operation = self.descriptor.WriteValueAsync(&buffer)?;
            drop(buffer);
            let res = operation.into_future().await;
            match res {
                Ok(result) => {
                    if result == GattCommunicationStatus::Success {
                        return Ok(());
                    } else {
                        return Err(Error::Other(
                            format!("Windows UWP threw error on write descriptor: {:?}", result).into(),
                        ));
                    }
                }
                Err(err) if attempts == 0 && utils::is_encryption_error(&err) => {
                    attempts += 1;
                    if let Err(pair_err) = utils::pair_from_characteristic(&self.characteristic).await {
                        log::warn!("Auto-pairing failed during write descriptor: {:?}", pair_err);
                        return Err(Error::from(err));
                    }
                    continue;
                }
                Err(err) => {
                    return Err(Error::from(err));
                }
            }
        }
    }

    pub async fn read_value(&self) -> Result<Vec<u8>> {
        let mut attempts = 0;
        loop {
            let res = self
                .descriptor
                .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?
                .into_future()
                .await;
            match res {
                Ok(result) => {
                    if result.Status()? == GattCommunicationStatus::Success {
                        let value = result.Value()?;
                        let reader = DataReader::FromBuffer(&value)?;
                        let len = reader.UnconsumedBufferLength()? as usize;
                        let mut input = vec![0u8; len];
                        reader.ReadBytes(&mut input[0..len])?;
                        return Ok(input);
                    } else {
                        return Err(Error::Other(
                            format!("Windows UWP threw error on read: {:?}", result).into(),
                        ));
                    }
                }
                Err(err) if attempts == 0 && utils::is_encryption_error(&err) => {
                    attempts += 1;
                    if let Err(pair_err) = utils::pair_from_characteristic(&self.characteristic).await {
                        log::warn!("Auto-pairing failed during read descriptor: {:?}", pair_err);
                        return Err(Error::from(err));
                    }
                    continue;
                }
                Err(err) => {
                    return Err(Error::from(err));
                }
            }
        }
    }
}
