ISO_VER=$(shell grep '^version' ../../Cargo.toml | cut -d'"' -f2)

DST_PKG_DIR=config/includes.chroot
SRC_PKG_DIR=../../target/debian

export USBSAS_ISO_VER=$(ISO_VER)
export USBSAS_BIN_PATH=/usr/libexec
export USBSAS_WEBFILES_DIR=/usr/share/usbsas/web

CORE_VER := $(shell grep '^version = ' ../../usbsas-usbsas/Cargo.toml | cut -d'"' -f2)
SRV_VER := $(shell grep '^version = ' ../../usbsas-server/Cargo.toml | cut -d'"' -f2)
ASRV_VER := $(shell grep '^version = ' ../../usbsas-analyzer-server/Cargo.toml | cut -d'"' -f2)
HID_VER := $(shell grep '^version = ' ../../usbsas-hid/hid-dealer/Cargo.toml | cut -d'"' -f2)
KIOSK_VER := $(shell grep 'Version' ../kiosk/usbsas-kiosk/DEBIAN/control | cut -d' ' -f2)

all: usbsas-$(ISO_VER)-amd64.hybrid.iso

pkgs: $(DST_PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-core_$(CORE_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb

$(DST_PKG_DIR)/%.deb: $(SRC_PKG_DIR)/%.deb
	cp $< $@

$(SRC_PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb:
	make -C ../kiosk

$(SRC_PKG_DIR)/usbsas-core_$(CORE_VER)_amd64.deb:
	cargo build --release --manifest-path=../../Cargo.toml
	cargo-deb --no-build --manifest-path=../../usbsas-usbsas/Cargo.toml

$(SRC_PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb:
	cargo-deb --no-build --manifest-path=../../usbsas-server/Cargo.toml

$(SRC_PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb:
	cargo build --release --manifest-path=../../usbsas-analyzer-server/Cargo.toml
	cargo-deb --no-build --manifest-path=../../usbsas-analyzer-server/Cargo.toml

$(SRC_PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb:
	cargo build --release --manifest-path=../../usbsas-hid/hid-user/Cargo.toml
	cargo build --release --manifest-path=../../usbsas-hid/hid-dealer/Cargo.toml
	cargo-deb --no-build --manifest-path=../../usbsas-hid/hid-dealer/Cargo.toml

usbsas-$(ISO_VER)-amd64.hybrid.iso: pkgs
	lb config
	sudo lb build

clean:
	sudo lb clean

distclean: clean
	sudo rm -rf cache
	rm $(DST_PKG_DIR)/*.deb
	make -C ../kiosk clean
	cargo clean --manifest-path=../../Cargo.toml
