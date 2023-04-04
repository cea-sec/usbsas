import os
import struct

from proto.usbsas import proto3_pb2 as proto_usbsas
from proto.common import proto3_pb2 as proto_common

class Comm(object):

    resp_types = {}
    req_types = {}
    response_cls = None
    request_cls = None

    def __init__(self, pipe_recv, pipe_send):
        self.pipe_send = pipe_send
        self.pipe_recv = pipe_recv

    def recv(self):
        data_size_b = os.read(self.pipe_recv, 8)
        data_size = struct.unpack('<Q', data_size_b)[0]
        data = b''
        while len(data) != data_size:
            data += os.read(self.pipe_recv, data_size)
        return data

    def send(self, buf):
        data_size_b = struct.pack('<Q', len(buf))
        os.write(self.pipe_send, data_size_b)
        os.write(self.pipe_send, buf)

    def send_msg(self, msg):
        buf = msg.SerializeToString()
        self.send(buf)

    def recv_resp(self):
        data = self.recv()
        resp = self.response_cls.FromString(data)
        subtype = resp.WhichOneof("msg")
        cls = self.resp_types.get(subtype)
        if cls is None:
            raise TypeError("Unknown response type for %r" % resp)
        resp = self.response_cls.FromString(data)
        return getattr(resp, subtype)

    def recv_req(self):
        data = self.recv()
        req = self.request_cls.FromString(data)
        subtype = req.WhichOneof("msg")
        return getattr(req, subtype)

    def send_resp(self, resp):
        type_str = {v:k for k, v in self.resp_types.items()}.get(resp.__class__)
        if type_str is None:
            raise TypeError("Unknown response type for %r" % resp)
        self.send_msg(self.response_cls(**{type_str: resp}))

    def send_req(self, req):
        type_str = {v:k for k, v in self.req_types.items()}.get(req.__class__)
        if type_str is None:
            raise TypeError("Unknown request type for %r" % req)
        self.send_msg(self.request_cls(**{type_str: req}))


class CommUsbsas(Comm):
    req_types = {
        "CopyStart": proto_usbsas.RequestCopyStart,
        "Devices": proto_usbsas.RequestDevices,
        "End": proto_usbsas.RequestEnd,
        "GetAttr": proto_usbsas.RequestGetAttr,
        "Id": proto_usbsas.RequestId,
        "ImgDisk": proto_usbsas.RequestImgDisk,
        "OpenDevice": proto_usbsas.RequestOpenDevice,
        "OpenPartition": proto_usbsas.RequestOpenPartition,
        "Partitions": proto_usbsas.RequestPartitions,
        "ReadDir": proto_usbsas.RequestReadDir,
        "Report": proto_usbsas.RequestReport,
        "Wipe": proto_usbsas.RequestWipe,
    }
    resp_types = {
        "AnalyzeDone": proto_usbsas.ResponseAnalyzeDone,
        "AnalyzeStatus": proto_usbsas.ResponseAnalyzeStatus,
        "CopyDone": proto_usbsas.ResponseCopyDone,
        "CopyStart": proto_usbsas.ResponseCopyStart,
        "CopyStatus": proto_usbsas.ResponseCopyStatus,
        "CopyStatusDone": proto_usbsas.ResponseCopyStatusDone,
        "Devices": proto_usbsas.ResponseDevices,
        "End": proto_usbsas.ResponseEnd,
        "Error": proto_usbsas.ResponseError,
        "FinalCopyStatus": proto_usbsas.ResponseFinalCopyStatus,
        "FinalCopyStatusDone": proto_usbsas.ResponseFinalCopyStatusDone,
        "GetAttr": proto_usbsas.ResponseGetAttr,
        "Id": proto_usbsas.ResponseId,
        "ImgDisk": proto_usbsas.ResponseImgDisk,
        "NotEnoughSpace": proto_usbsas.ResponseNotEnoughSpace,
        "OpenDevice": proto_usbsas.ResponseOpenDevice,
        "OpenPartition": proto_usbsas.ResponseOpenPartition,
        "Partitions": proto_usbsas.ResponsePartitions,
        "PostCopyCmd": proto_usbsas.ResponsePostCopyCmd,
        "ReadDir": proto_usbsas.ResponseReadDir,
        "Report": proto_usbsas.ResponseReport,
        "Wipe": proto_usbsas.ResponseWipe,
    }
    response_cls = proto_usbsas.Response
    request_cls = proto_usbsas.Request

    def is_error(self, resp):
        return isinstance(resp, proto_usbsas.ResponseError)

    def get_file_attr(self, path):
        self.send_req(proto_usbsas.RequestGetAttr(path=path))
        return self.recv_resp()

    def end(self):
        self.send_req(proto_usbsas.RequestEnd())
        return self.recv_resp()

    def id(self):
        self.send_req(proto_usbsas.RequestId())
        return self.recv_resp()

    def devices(self):
        self.send_req(proto_usbsas.RequestDevices())
        return self.recv_resp()

    def open_device(self, busnum, devnum):
        self.send_req(proto_usbsas.RequestOpenDevice(
            device=proto_common.Device(busnum=busnum, devnum=devnum)
            ))
        return self.recv_resp()

    def partitions(self):
        self.send_req(proto_usbsas.RequestPartitions())
        return self.recv_resp()

    def open_partition(self, index):
        self.send_req(proto_usbsas.RequestOpenPartition(index=index))
        return self.recv_resp()

    def read_dir(self, path):
        self.send_req(proto_usbsas.RequestReadDir(path=path))
        return self.recv_resp()

    def copy_files_usb(self, selected, busnum, devnum):
        req = proto_usbsas.RequestCopyStart(selected=selected)
        req.usb.busnum = busnum
        req.usb.devnum = devnum
        req.usb.fstype = proto_usbsas.NTFS
        req.src_usb.SetInParent()
        self.send_req(req)
        return self.recv_resp()

    def copy_files_net(self, selected, url):
        req = proto_usbsas.RequestCopyStart(selected=selected)
        req.net.url = url
        req.src_usb.SetInParent()
        self.send_req(req)
        return self.recv_resp()

    def wipe(self, busnum, devnum, fstype, quick):
        self.send_req(proto_usbsas.RequestWipe(
            busnum=busnum, devnum=devnum, fstype=fstype, quick=quick
            ))

    def imgdisk(self, busnum, devnum):
        self.send_req(proto_usbsas.RequestImgDisk(
            device=proto_common.Device(busnum=busnum, devnum=devnum)
            ))
        return self.recv_resp()

    def report(self):
        self.send_req(proto_usbsas.RequestReport())
        return self.recv_resp()
