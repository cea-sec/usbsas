# usbsas_transfer_example.py

Simple python script to copy files from a USB device to another using usbsas.

/!\\ Don't use this as is in production, it is roughly an example of how to use
usbsas with python and protobuf.

This script waits for 2 USB devices to be connected (1st one will be the source,
2nd the destination, which will be overwritten), and copy all files from the
1st device to the 2nd.

## Usage
Generate protobuf python code (with `make`), start the configured analyzer-server and run the script.

```
$ make
$ python usbsas_transfer_example.py
```

## Dependencies:
* make
* protoc
* python-protobuf
