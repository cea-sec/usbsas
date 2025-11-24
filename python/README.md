# usbsas Python bindings

## Prerequisite

Install dependencies:

* make
* protoc
* python-protobuf

Then generate python protobuf code:

```
$ make
```


## Example

Example using communication wrapper from `comm.py`

```bash
$ usbsas-usbsas -s
```

```python
>>> import socket
>>> from comm import CommUsbsas
>>> socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
>>> socket.connect('/tmp/usbsas.sock')
>>> # CommUsbsas expects one file descriptor for reading and one for writing (it was initially written for pipes) but since a socket is bidirectional you can use same fd for both
>>> comm = CommUsbsas(socket.fileno(), socket.fileno())
>>> comm.devices()
devices {
  network {
    url: "http://127.0.0.1:8042/api/downloadbundle"
    title: "Source Network XXX"
    description: "Export files from network XXX"
    is_src: true
  }
  id: 11434200041057920751
}
devices {
  usb {
    busnum: 1
    devnum: 2
    vendorid: 1
    productid: 1
    manufacturer: "manufacturer"
    description: "mock output dev"
    serial: "serial"
    is_dst: true
  }
  id: 15900793998882151938
}
devices {
  usb {
    busnum: 1
    devnum: 1
    vendorid: 1
    productid: 1
    manufacturer: "manufacturer"
    description: "mock input dev"
    serial: "serial"
    is_src: true
  }
  id: 4900546889350061835
}
devices {
  network {
    url: "http://127.0.0.1:8042/api/uploadbundle"
    krb_service_name: "HTTP@your.domain"
    title: "Network XXX"
    description: "Send files on network XXX"
    is_dst: true
  }
  id: 1677896787701198402
}
devices {
  command {
    bin: "/bin/cp"
    args: "%SOURCE_FILE%"
    args: "/usbsas_data"
    title: "Save files on disk"
    description: "Save out tar in /usbsas_data/"
    is_dst: true
  }
  id: 9858991577920643068
}
>>> # Let's transfer files from USB device to USB device
>>> comm.userid()
>>> comm.init_transfer(4900546889350061835, 15900793998882151938)
>>> comm.partitions()
partitions {
  size: 8388608
  start: 2048
  ptype: 7
  name_str: "Unknown"
  type_str: "NTFS"
}
partitions {
  size: 8388608
  start: 18432
  ptype: 11
  name_str: "Unknown"
  type_str: "FAT"
}
partitions {
  size: 8388608
  start: 34816
  ptype: 131
  name_str: "tset"
  type_str: "Linux/Ext"
}
>>> comm.open_partition(0)
>>> comm.read_dir("/")
filesinfo {
  path: "/AUTORUN.INF"
  ftype: REGULAR
  timestamp: 1742982547
}
filesinfo {
  path: "/chicken.pdf"
  ftype: REGULAR
  size: 51500
  timestamp: 1742982547
}
filesinfo {
  path: "/infected"
  ftype: DIRECTORY
  timestamp: 1742982547
}
filesinfo {
  path: "/Micro$oft.lnk"
  ftype: REGULAR
  timestamp: 1742982547
}
filesinfo {
  path: "/quiche"
  ftype: DIRECTORY
  timestamp: 1742982547
}
filesinfo {
  path: "/tree 🌲.txt"
  ftype: REGULAR
  size: 721
  timestamp: 1742982547
}
filesinfo {
  path: "/usbsas-logo.svg"
  ftype: REGULAR
  size: 13113
  timestamp: 1742982547
}
>>> comm.read_dir("/quiche")
filesinfo {
  path: "/quiche/lorem ipsum.txt"
  ftype: REGULAR
  size: 6223
  timestamp: 1742982547
}
filesinfo {
  path: "/quiche/.DS_STORE"
  ftype: REGULAR
  timestamp: 1742982547
}
filesinfo {
  path: "/quiche/plop"
  ftype: DIRECTORY
  timestamp: 1742982547
>>> comm.select_files(selected=["/chicken.pdf", "/quiche"])
>>> comm.status()
READ_SRC : 51500 / 1120201
READ_SRC : 51568 / 1120201
READ_SRC : 57791 / 1120201
READ_SRC : 1106367 / 1120201
READ_SRC : 1107088 / 1120201
READ_SRC : 1120201 / 1120201
READ_SRC : 1120201 / 1120201
UPLOAD_AV : 81920 / 1129984
UPLOAD_AV : 163840 / 1129984
UPLOAD_AV : 245760 / 1129984
UPLOAD_AV : 327680 / 1129984
UPLOAD_AV : 409600 / 1129984
UPLOAD_AV : 491520 / 1129984
UPLOAD_AV : 573440 / 1129984
UPLOAD_AV : 655360 / 1129984
UPLOAD_AV : 737280 / 1129984
UPLOAD_AV : 819200 / 1129984
UPLOAD_AV : 901120 / 1129984
UPLOAD_AV : 983040 / 1129984
UPLOAD_AV : 1064960 / 1129984
UPLOAD_AV : 1129984 / 1129984
UPLOAD_AV : 0 / 0
ANALYZE : 0 / 0
ANALYZE : 0 / 0
MK_FS : 51500 / 1120201
MK_FS : 57723 / 1120201
MK_FS : 1106299 / 1120201
MK_FS : 1107020 / 1120201
MK_FS : 1120133 / 1120201
MK_FS : 0 / 0
WRITE_DST : 512 / 3744768
WRITE_DST : 12800 / 3744768
WRITE_DST : 90624 / 3744768
WRITE_DST : 213504 / 3744768
WRITE_DST : 336384 / 3744768
WRITE_DST : 459264 / 3744768
WRITE_DST : 524800 / 3744768
WRITE_DST : 528896 / 3744768
WRITE_DST : 651776 / 3744768
WRITE_DST : 774656 / 3744768
WRITE_DST : 897536 / 3744768
WRITE_DST : 1020416 / 3744768
WRITE_DST : 1143296 / 3744768
WRITE_DST : 1266176 / 3744768
WRITE_DST : 1389056 / 3744768
WRITE_DST : 1511936 / 3744768
WRITE_DST : 1634816 / 3744768
WRITE_DST : 1757696 / 3744768
WRITE_DST : 1880576 / 3744768
WRITE_DST : 2003456 / 3744768
WRITE_DST : 2126336 / 3744768
WRITE_DST : 2249216 / 3744768
WRITE_DST : 2372096 / 3744768
WRITE_DST : 2494976 / 3744768
WRITE_DST : 2617856 / 3744768
WRITE_DST : 2683392 / 3744768
WRITE_DST : 2691584 / 3744768
WRITE_DST : 2814464 / 3744768
WRITE_DST : 2937344 / 3744768
WRITE_DST : 3060224 / 3744768
WRITE_DST : 3183104 / 3744768
WRITE_DST : 3305984 / 3744768
WRITE_DST : 3428864 / 3744768
WRITE_DST : 3551744 / 3744768
WRITE_DST : 3674624 / 3744768
WRITE_DST : 3740160 / 3744768
WRITE_DST : 3744256 / 3744768
WRITE_DST : 3744768 / 3744768
WRITE_DST : 3744768 / 3744768
ALL_DONE : 0 / 0
>>> comm.report()
title: "usbsas_transfer_20250416064926626_b622fe4c89cb4d3389d1d390804b5101"
datetime: "20250416064926626"
timestamp: 1744786166
hostname: "computer"
status: "success"
user: "Tartempion"
transfer_id: "b622fe4c89cb4d3389d1d390804b5101"
source {
  usb {
    vendorid: 1
    productid: 1
    manufacturer: "manufacturer"
    description: "mock input dev"
    serial: "serial"
  }
}
destination {
  usb {
    vendorid: 1
    productid: 1
    manufacturer: "manufacturer"
    description: "mock output dev"
    serial: "serial"
  }
}
file_names: "/chicken.pdf"
file_names: "/quiche/lorem ipsum.txt"
file_names: "/quiche/plop/random.bin"
file_names: "/tree 🌲.txt"
file_names: "/usbsas-logo.svg"
filtered_files: "/AUTORUN.INF"
filtered_files: "/Micro$oft.lnk"
filtered_files: "/quiche/.DS_STORE"
rejected_files: "infected/eicar.com"
analyzereport {
  id: "a8c1d22fce994e418bab184f4dfcd3d8"
  status: "scanned"
  version: 2
  antivirus {
    key: "ClamAV"
    value {
      version: "ClamAV 1.4.2"
      database_version: "27582"
      database_timestamp: "Wed Mar 19 10:41:59 2025"
    }
  }
  files {
    key: "usbsas-logo.svg"
    value {
      status: "CLEAN"
    }
  }
  files {
    key: "tree 🌲.txt"
    value {
      status: "CLEAN"
    }
  }
  files {
    key: "quiche/plop/random.bin"
    value {
      status: "CLEAN"
    }
  }
  files {
    key: "quiche/lorem ipsum.txt"
    value {
      status: "CLEAN"
    }
  }
  files {
    key: "infected/eicar.com"
    value {
      status: "DIRTY"
    }
  }
  files {
    key: "chicken.pdf"
    value {
      status: "CLEAN"
    }
  }
}

>>> comm.end()
>>> socket.close()
```
