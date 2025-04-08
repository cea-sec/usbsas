import os
import sys
import struct

from proto.usbsas import proto3_pb2 as proto_usbsas
from proto.common import proto3_pb2 as proto_common


class Comm():

    resp_types = {}
    req_types = {}
    response_cls = None
    request_cls = None

    def __init__(self, pipe_recv, pipe_send):
        self.pipe_send = pipe_send
        self.pipe_recv = pipe_recv

    def recv(self):
        data_size_b = os.read(self.pipe_recv, 8)
        data_size = struct.unpack("<Q", data_size_b)[0]
        data = b""
        while len(data) != data_size:
            data += os.read(self.pipe_recv, data_size)
        return data

    def send(self, buf):
        data_size_b = struct.pack("<Q", len(buf))
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
        "OpenDevice": proto_usbsas.RequestOpenDevice,
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
        "OpenDevice": proto_usbsas.ResponseOpenDevice,
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
                print(f"error: {resp.error}")
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

    def open_device(self, device):
        self.send_req(proto_usbsas.RequestOpenDevice(device=device))
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
        if not isinstance(resp, proto_usbsas.ResponseImgDisk):
            raise Exception(f"ImgDisk error: {resp.error}")


def start_usbsas():
    usbsas_bin = os.path.join(
        os.path.dirname(os.path.realpath(__file__)), "../target/debug/usbsas-usbsas"
    )
    config_path = os.path.join(
        os.path.dirname(os.path.realpath(__file__)), "../config.example.toml"
    )
    if not os.path.exists(usbsas_bin):
        print("usbsas-usbsas binary not found")
        sys.exit(1)
    (child_to_parent_r, child_to_parent_w) = os.pipe()
    (parent_to_child_r, parent_to_child_w) = os.pipe()
    os.set_inheritable(child_to_parent_w, True)
    os.set_inheritable(parent_to_child_r, True)
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
        os.environ["USBSAS_MOCK_OUT_DEV"] = "/tmp/mock_out_dev.img"
        os.environ["USBSAS_MOCK_IN_DEV"] = "/tmp/mock_in_dev.img"
        os.execv(usbsas_bin, [usbsas_bin, "-c", config_path])
        sys.exit(0)
    os.close(parent_to_child_r)
    os.close(child_to_parent_w)
    comm = CommUsbsas(child_to_parent_r, parent_to_child_w)
    return comm
