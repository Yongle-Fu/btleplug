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

use super::{super::utils::to_descriptor_value, descriptor::BLEDescriptor};
use crate::{
    Error, Result,
    api::{Characteristic, WriteType},
    winrtble::utils,
};

use log::{debug, trace};
use std::{collections::HashMap, future::IntoFuture};
use uuid::Uuid;
use windows::core::Ref;
use windows::{
    Devices::Bluetooth::{
        BluetoothCacheMode,
        GenericAttributeProfile::{
            GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
            GattCommunicationStatus, GattValueChangedEventArgs, GattWriteOption,
        },
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataReader, DataWriter},
};

pub type NotifiyEventHandler = Box<dyn Fn(Vec<u8>) + Send>;

impl From<WriteType> for GattWriteOption {
    fn from(val: WriteType) -> Self {
        match val {
            WriteType::WithoutResponse => GattWriteOption::WriteWithoutResponse,
            WriteType::WithResponse => GattWriteOption::WriteWithResponse,
        }
    }
}

#[derive(Debug)]
pub struct BLECharacteristic {
    characteristic: GattCharacteristic,
    pub descriptors: HashMap<Uuid, BLEDescriptor>,
    notify_token: Option<i64>,
}

impl BLECharacteristic {
    pub fn new(
        characteristic: GattCharacteristic,
        descriptors: HashMap<Uuid, BLEDescriptor>,
    ) -> Self {
        BLECharacteristic {
            characteristic,
            descriptors,
            notify_token: None,
        }
    }

    pub async fn write_value(&self, data: &[u8], write_type: WriteType) -> Result<()> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(data)?;
        let buffer = writer.DetachBuffer()?;
        let mut attempts = 0;
        loop {
            let operation = self
                .characteristic
                .WriteValueWithOptionAsync(&buffer, write_type.into())?;
            let res = operation.into_future().await;
            match res {
                Ok(result) => {
                    if result == GattCommunicationStatus::Success {
                        return Ok(());
                    } else {
                        return Err(Error::Other(
                            format!("Windows UWP threw error on write: {:?}", result).into(),
                        ));
                    }
                }
                Err(err) if attempts == 0 && utils::is_encryption_error(&err) => {
                    attempts += 1;
                    if let Err(pair_err) = utils::pair_from_characteristic(&self.characteristic).await {
                        log::warn!("Auto-pairing failed during write: {:?}", pair_err);
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
                .characteristic
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
                        log::warn!("Auto-pairing failed during read: {:?}", pair_err);
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

    pub async fn subscribe(&mut self, on_value_changed: NotifiyEventHandler) -> Result<()> {
        {
            let value_handler = TypedEventHandler::new(
                move |_: Ref<GattCharacteristic>, args: Ref<GattValueChangedEventArgs>| {
                    if let Ok(args) = args.ok() {
                        let value = args.CharacteristicValue()?;
                        let reader = DataReader::FromBuffer(&value)?;
                        let len = reader.UnconsumedBufferLength()? as usize;
                        let mut input: Vec<u8> = vec![0u8; len];
                        reader.ReadBytes(&mut input[0..len])?;
                        trace!("changed {:?}", input);
                        on_value_changed(input);
                    }
                    Ok(())
                },
            );
            let token = self.characteristic.ValueChanged(&value_handler)?;
            self.notify_token = Some(token);
        }
        let config = to_descriptor_value(self.characteristic.CharacteristicProperties()?);
        if config == GattClientCharacteristicConfigurationDescriptorValue::None {
            return Err(Error::NotSupported("Can not subscribe to attribute".into()));
        }

        let mut attempts = 0;
        loop {
            let res = self
                .characteristic
                .WriteClientCharacteristicConfigurationDescriptorAsync(config)?
                .into_future()
                .await;
            match res {
                Ok(status) => {
                    trace!("subscribe {:?}", status);
                    if status == GattCommunicationStatus::Success {
                        return Ok(());
                    } else {
                        return Err(Error::Other(
                            format!("Windows UWP threw error on subscribe: {:?}", status).into(),
                        ));
                    }
                }
                Err(err) if attempts == 0 && utils::is_encryption_error(&err) => {
                    attempts += 1;
                    if let Err(pair_err) = utils::pair_from_characteristic(&self.characteristic).await {
                        log::warn!("Auto-pairing failed during subscribe: {:?}", pair_err);
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

    pub async fn unsubscribe(&mut self) -> Result<()> {
        if let Some(token) = &self.notify_token {
            self.characteristic.RemoveValueChanged(*token)?;
        }
        self.notify_token = None;
        let config = GattClientCharacteristicConfigurationDescriptorValue::None;
        let status = self
            .characteristic
            .WriteClientCharacteristicConfigurationDescriptorAsync(config)?
            .into_future()
            .await?;
        trace!("unsubscribe {:?}", status);
        if status == GattCommunicationStatus::Success {
            Ok(())
        } else {
            Err(Error::Other(
                format!("Windows UWP threw error on unsubscribe: {:?}", status).into(),
            ))
        }
    }

    pub fn uuid(&self) -> Uuid {
        utils::to_uuid(&self.characteristic.Uuid().unwrap())
    }

    pub fn to_characteristic(&self, service_uuid: Uuid) -> Characteristic {
        let uuid = self.uuid();
        let properties =
            utils::to_char_props(&self.characteristic.CharacteristicProperties().unwrap());
        let descriptors = self
            .descriptors
            .values()
            .map(|descriptor| descriptor.to_descriptor(service_uuid, uuid))
            .collect();
        Characteristic {
            uuid,
            service_uuid,
            descriptors,
            properties,
        }
    }
}

impl Drop for BLECharacteristic {
    fn drop(&mut self) {
        if let Some(token) = &self.notify_token {
            let result = self.characteristic.RemoveValueChanged(*token);
            if let Err(err) = result {
                debug!("Drop:remove_connection_status_changed {:?}", err);
            }
        }
    }
}
