import os
import socket
import struct

from proto.usbsas import proto3_pb2 as proto_usbsas
from proto.common import proto3_pb2 as proto_common


class Comm():

    resp_types = {}
    req_types = {}
    response_cls = None
    request_cls = None

    def __init__(self, pipe_recv, pipe_send, sock=None):
        self.pipe_send = pipe_send
        self.pipe_recv = pipe_recv
        # Keep a reference, the socket would be closed when garbage collected
        self.sock = sock

    @classmethod
    def connect(cls, socket_path):
        """Connect to the Unix socket of a usbsas server (`usbsas-usbsas -s`)."""
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(socket_path)
        except OSError:
            sock.close()
            raise
        return cls(sock.fileno(), sock.fileno(), sock)

    def close(self):
        if self.sock is not None:
            self.sock.close()
            self.sock = None

    def read_exact(self, size):
        data = bytearray()
        while len(data) < size:
            chunk = os.read(self.pipe_recv, size - len(data))
            if not chunk:
                raise ConnectionError("connection closed by peer")
            data += chunk
        return bytes(data)

    def recv(self):
        data_size = struct.unpack("<Q", self.read_exact(8))[0]
        return self.read_exact(data_size)

    def write_all(self, buf):
        view = memoryview(buf)
        while view:
            written = os.write(self.pipe_send, view)
            view = view[written:]

    def send(self, buf):
        self.write_all(struct.pack("<Q", len(buf)) + buf)

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
        type_str = {v: k for k, v in self.resp_types.items()}.get(resp.__class__)
        if type_str is None:
            raise TypeError("Unknown response type for %r" % resp)
        self.send_msg(self.response_cls(**{type_str: resp}))

    def send_req(self, req):
        type_str = {v: k for k, v in self.req_types.items()}.get(req.__class__)
        if type_str is None:
            raise TypeError("Unknown request type for %r" % req)
        self.send_msg(self.request_cls(**{type_str: req}))


class CommUsbsas(Comm):
    req_types = {
        "Devices": proto_usbsas.RequestDevices,
        "End": proto_usbsas.RequestEnd,
        "GetAttr": proto_usbsas.RequestGetAttr,
        "ImgDisk": proto_usbsas.RequestImgDisk,
        "InitTransfer": proto_usbsas.RequestInitTransfer,
        "OpenPartition": proto_usbsas.RequestOpenPartition,
        "Partitions": proto_usbsas.RequestPartitions,
        "ReadDir": proto_usbsas.RequestReadDir,
        "SelectFiles": proto_usbsas.RequestSelectFiles,
        "Report": proto_usbsas.RequestReport,
        "UserId": proto_usbsas.RequestUserId,
        "Wipe": proto_usbsas.RequestWipe,
    }
    resp_types = {
        "Devices": proto_usbsas.ResponseDevices,
        "End": proto_usbsas.ResponseEnd,
        "Error": proto_usbsas.ResponseError,
        "GetAttr": proto_usbsas.ResponseGetAttr,
        "ImgDisk": proto_usbsas.ResponseImgDisk,
        "InitTransfer": proto_usbsas.ResponseInitTransfer,
        "OpenPartition": proto_usbsas.ResponseOpenPartition,
        "Partitions": proto_usbsas.ResponsePartitions,
        "ReadDir": proto_usbsas.ResponseReadDir,
        "SelectFiles": proto_usbsas.ResponseSelectFiles,
        "Report": proto_usbsas.ResponseReport,
        "Status": proto_usbsas.ResponseStatus,
        "UserId": proto_usbsas.ResponseUserId,
        "Wipe": proto_usbsas.ResponseWipe,
    }
    response_cls = proto_usbsas.Response
    request_cls = proto_usbsas.Request

    def is_error(self, resp):
        return isinstance(resp, proto_usbsas.ResponseError)

    def end(self):
        self.send_req(proto_usbsas.RequestEnd())
        return self.recv_resp()

    def status(self):
        while True:
            resp = self.recv_resp()
            if self.is_error(resp):
                print(f"error: {resp.err}")
                raise Exception("run error")
            if not isinstance(resp, proto_usbsas.ResponseStatus):
                raise Exception("run error")
            print(proto_usbsas.Status.Name(resp.status), f": {resp.current} / {resp.total}")
            if resp.status == proto_usbsas.Status.ALL_DONE:
                break

    def devices(self, include_alt=False):
        self.send_req(proto_usbsas.RequestDevices(include_alt=include_alt))
        return self.recv_resp()

    def userid(self):
        self.send_req(proto_usbsas.RequestUserId())
        return self.recv_resp()

    def init_transfer(self, src, dst, fstype=proto_common.FsType.NTFS, pin=None):
        self.send_req(
            proto_usbsas.RequestInitTransfer(
                source=src, destination=dst, fstype=fstype, pin=pin
            )
        )
        return self.recv_resp()

    def partitions(self):
        self.send_req(proto_usbsas.RequestPartitions())
        return self.recv_resp()

    def open_partition(self, index):
        self.send_req(proto_usbsas.RequestOpenPartition(index=index))
        return self.recv_resp()

    def get_file_attr(self, path):
        self.send_req(proto_usbsas.RequestGetAttr(path=path))
        return self.recv_resp()

    def read_dir(self, path):
        self.send_req(proto_usbsas.RequestReadDir(path=path))
        return self.recv_resp()

    def select_files(self, selected):
        self.send_req(proto_usbsas.RequestSelectFiles(selected=selected))
        return self.recv_resp()

    def report(self):
        self.send_req(proto_usbsas.RequestReport())
        resp = self.recv_resp()
        if isinstance(resp, proto_usbsas.ResponseReport):
            return resp.report
        else:
            raise Exception("Error getting report")

    def wipe(self, dev_id, fstype=proto_common.FsType.NTFS, quick=False):
        print(f"Wipe device {dev_id}")
        self.send_req(
            proto_usbsas.RequestWipe(
                id=dev_id, fstype=fstype, quick=quick
            )
        )
        return self.recv_resp()

    def imgdisk(self, dev_id):
        self.send_req(
            proto_usbsas.RequestImgDisk(
                id=dev_id
            )
        )
        resp = self.recv_resp()
        if self.is_error(resp):
            raise Exception(f"ImgDisk error: {resp.err}")
        if not isinstance(resp, proto_usbsas.ResponseImgDisk):
            raise Exception(f"ImgDisk: unexpected response {resp!r}")

