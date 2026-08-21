package com.nonpolynomial.btleplug.android.impl;

import android.annotation.SuppressLint;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanFilter.Builder;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.os.Build;
import android.os.ParcelUuid;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

@SuppressWarnings("unused") // Native code uses this class.
class Adapter {
    private long handle;
    private final Callback callback = new Callback();

    public Adapter() {}

    @SuppressLint("MissingPermission")
    public void startScan(ScanFilter filter) {
        BluetoothAdapter bluetoothAdapter = BluetoothAdapter.getDefaultAdapter();
        if (bluetoothAdapter == null) {
          throw new NoBluetoothAdapterException();
        }

        ArrayList<android.bluetooth.le.ScanFilter> filters = null;
        String[] uuids = filter.getUuids();
        if (uuids.length > 0) {
            filters = new ArrayList<>();
            for (String uuid : uuids) {
                filters.add(new Builder().setServiceUuid(ParcelUuid.fromString(uuid)).build());
            }
        }
        ScanSettings settings;
        if (Build.VERSION.SDK_INT >= 26) {
            settings = new ScanSettings.Builder()
                    .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
                    .setLegacy(false)
                    .build();
        } else {
            settings = new ScanSettings.Builder()
                    .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
                    .build();
        }
        BluetoothLeScanner scanner = bluetoothAdapter.getBluetoothLeScanner();
        if (scanner == null) {
          throw new NoBluetoothAdapterException();
        }
        scanner.startScan(filters, settings, this.callback);
    }

    @SuppressLint("MissingPermission")
    public void startScanWithTransport(ScanFilter filter, int transport) {
        if (transport != BluetoothDevice.TRANSPORT_AUTO &&
                transport != BluetoothDevice.TRANSPORT_BREDR &&
                transport != BluetoothDevice.TRANSPORT_LE) {
            throw new IllegalArgumentException("Invalid GATT transport: " + transport);
        }
        if (transport != BluetoothDevice.TRANSPORT_BREDR) {
            startScan(filter);
        }
        if (transport != BluetoothDevice.TRANSPORT_LE) {
            reportBondedClassicDevices(filter);
        }
    }

    @SuppressLint("MissingPermission")
    private void reportBondedClassicDevices(ScanFilter filter) {
        BluetoothAdapter bluetoothAdapter = BluetoothAdapter.getDefaultAdapter();
        if (bluetoothAdapter == null) {
            throw new NoBluetoothAdapterException();
        }

        Set<String> requestedUuids = new HashSet<>();
        for (String uuid : filter.getUuids()) {
            requestedUuids.add(uuid.toLowerCase());
        }
        for (BluetoothDevice device : bluetoothAdapter.getBondedDevices()) {
            ParcelUuid[] parcelUuids = device.getUuids();
            ArrayList<String> serviceUuids = new ArrayList<>();
            boolean matches = requestedUuids.isEmpty();
            if (parcelUuids != null) {
                for (ParcelUuid parcelUuid : parcelUuids) {
                    String uuid = parcelUuid.getUuid().toString();
                    serviceUuids.add(uuid);
                    if (requestedUuids.contains(uuid.toLowerCase())) {
                        matches = true;
                    }
                }
            }
            if (matches) {
                reportBondedDevice(
                        device.getAddress(),
                        device.getName() == null ? "" : device.getName(),
                        serviceUuids.toArray(new String[0]));
            }
        }
    }

    @SuppressLint("MissingPermission")
    public void stopScan() {
        BluetoothAdapter bluetoothAdapter = BluetoothAdapter.getDefaultAdapter();
        if (bluetoothAdapter != null) {
            BluetoothLeScanner scanner = bluetoothAdapter.getBluetoothLeScanner();
            if (scanner != null) {
                scanner.stopScan(this.callback);
            }
        }
    }

    private native void reportScanResult(ScanResult result);

    private native void reportBondedDevice(String address, String name, String[] serviceUuids);

    public native void onConnectionStateChanged(String address, boolean connected);

    private class Callback extends ScanCallback {
        @Override
        public void onScanResult(int callbackType, ScanResult result) {
            Adapter.this.reportScanResult(result);
        }
    }
}
