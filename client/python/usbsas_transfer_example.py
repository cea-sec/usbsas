#! /usr/bin/env python
# -*- coding: utf-8 -*-

"""
Simple python script to copy files from a USB device to another using usbsas.

(Start usbsas-analyzer-server before running this script or remove "--analyze"
flag in execv bellow)

/!\ Don't use this as is in production, it is roughly an example of how to use
    usbsas with python and protobuf
"""

import datetime
import os
import signal
import struct
import sys
import time

from comm import CommUsbsas
from proto.usbsas import proto3_pb2 as proto_usbsas

usbsas_bin = "/usr/libexec/usbsas-usbsas"
date = datetime.datetime.now()
out_tar = "/tmp/usbsas_tmp_%s.tar" % str(date).replace(' ', '_')
out_fs = "/tmp/usbsas_tmp_%s.fs" % str(date).replace(' ', '_')
pid_usbsas = -1

if not os.path.exists(usbsas_bin):
    usbsas_bin = "../../target/release/usbsas-usbsas"
    if not os.path.exists(usbsas_bin):
        print("usbsas-usbsas binary not found")
        sys.exit(1)


def start_usbsas():
    global pid_usbsas
    (child_to_parent_r, child_to_parent_w) = os.pipe()
    (parent_to_child_r, parent_to_child_w) = os.pipe()
    os.set_inheritable(child_to_parent_w, True)
    os.set_inheritable(parent_to_child_r, True)
    with open(out_tar, mode='w'): pass
    with open(out_fs, mode='w'): pass
    pid_usbsas = os.fork()
    if pid_usbsas < 0:
        print("fork error")
        sys.exit(1)
    if pid_usbsas == 0:
        # Should be closed already (non-inheritable)
        os.close(child_to_parent_r)
        os.close(parent_to_child_w)
        os.environ["INPUT_PIPE_FD"] = str(parent_to_child_r)
        os.environ["OUTPUT_PIPE_FD"] = str(child_to_parent_w)
        os.environ["RUST_LOG"] = "error"
        os.execv(usbsas_bin, [usbsas_bin, out_tar, out_fs, "--analyze"])
        sys.exit(0)
    os.close(parent_to_child_r)
    os.close(child_to_parent_w)
    comm = CommUsbsas(child_to_parent_r, parent_to_child_w)
    return comm

def wait_2_devices(comm):
    print("Waiting 2 devices (1st as source, 2nd as destination)")
    devices = []
    while True:
        rep = comm.devices()
        ok_or_exit(comm, rep, "error getting devices")
        if len(rep.devices) > 0:
            print("CONNECTED DEVICES:")
            for dev in rep.devices:
                print(devstr(dev))
        if len(rep.devices) == 2:
            return rep.devices
        time.sleep(1)

def open_dev_and_part(comm, device):
    print("Opening first partition of first device")
    rep = comm.open_device(device.busnum, device.devnum)
    ok_or_exit(comm, rep, "error opening device")
    rep = comm.partitions()
    ok_or_exit(comm, rep, "error reading partitions")
    print("Opening part")
    rep = comm.open_partition(index=0)
    ok_or_exit(comm, rep, "error opening first partition")

def list_files(comm):
    rep = comm.read_dir(path="/")
    ok_or_exit(comm, rep, "error listing files")
    return [f.path for f in rep.filesinfo]

def copy(comm, files, device):
    rep = comm.copy_files_usb(selected=files, busnum=device.busnum, devnum=device.devnum)
    ok_or_exit(comm, rep, "error starting copy")
    print("Starting copy")
    while True:
        rep = comm.recv_resp()
        ok_or_exit(comm, rep, "error during copy")
        if isinstance(rep, proto_usbsas.ResponseCopyDone):
            print("Transfer done, report:")
            print(rep)
            return
        print(rep)

def confirm_copy(devices):
    print('Copy all files from \n\"{}\"\nto\n\"{}\"\n? [Y/n]'.format(
        devstr(devices[0]), devstr(devices[1])
        ))
    if input().lower() == 'y':
        return True
    else:
        return False

def devstr(device):
    return '{0.manufacturer} {0.description} - {0.serial} ({0.vendorid}-{0.productid})'.format(device)

def ok_or_exit(comm, rep, mess):
    if comm.is_error(rep):
        print(mess)
        end(comm)

def end(comm):
    comm.end()
    os.kill(pid_usbsas, signal.SIGTERM)
    os.waitpid(pid_usbsas, 0)
    os.remove(out_tar)
    os.remove(out_fs)
    sys.exit(0)

def main():
    comm = start_usbsas()
    devices = wait_2_devices(comm)
    open_dev_and_part(comm, devices[0])
    files = list_files(comm)
    comm.id()
    if confirm_copy(devices):
        copy(comm, files, devices[1])
    end(comm)
    sys.exit(0)


if __name__ == '__main__':
    main()
