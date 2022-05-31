all: proto/usbsas/proto3_pb2.py proto/common/proto3_pb2.py

proto/%/proto3_pb2.py: ../../usbsas-proto/proto/%.proto3
	mkdir -p proto
	protoc  --proto_path=../../usbsas-proto/proto \
		--python_out=proto \
		$<
	# fix import
	sed -i 's/from\ common/from\ proto.common/g' proto/usbsas/proto3_pb2.py

clean:
	rm -rf proto __pycache__
