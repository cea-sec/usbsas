ISO_VER=$(shell grep '^version' ../Cargo.toml | cut -d'"' -f2)

PKG_DIR=../target/debian

export USBSAS_ISO_VER=$(ISO_VER)
export USBSAS_BIN_PATH=/usr/libexec

CORE_VER := $(shell grep '^version = ' ../usbsas-usbsas/Cargo.toml | cut -d'"' -f2)-1
SRV_VER := $(shell grep '^version = ' ../usbsas-server/Cargo.toml | cut -d'"' -f2)-1
ASRV_VER := $(shell grep '^version = ' ../usbsas-analyzer-server/Cargo.toml | cut -d'"' -f2)-1
HID_VER := $(shell grep '^version = ' ../usbsas-hid/hid-dealer/Cargo.toml | cut -d'"' -f2)-1
KIOSK_VER := $(shell grep 'Version' ../client/kiosk/usbsas-kiosk/DEBIAN/control | cut -d' ' -f2)-1

all: ../live-iso/usbsas-$(ISO_VER)-amd64.hybrid.iso

pkgs: $(PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb \
	$(PKG_DIR)/usbsas-core_$(CORE_VER)_amd64.deb \
	$(PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb \
	$(PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb \
	$(PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb

$(PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb:
	cargo build --release --manifest-path=../usbsas-client/Cargo.toml
	cargo-deb --no-build --manifest-path=../usbsas-client/Cargo.toml

$(PKG_DIR)/usbsas-core_$(CORE_VER)_amd64.deb:
	cargo build --release --manifest-path=../Cargo.toml
	cargo-deb --no-build --manifest-path=../usbsas-usbsas/Cargo.toml

$(PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb:
	cargo build --release --manifest-path=../usbsas-server/Cargo.toml
	cargo-deb --no-build --manifest-path=../usbsas-server/Cargo.toml

$(PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb:
	cargo build --release --manifest-path=../usbsas-analyzer-server/Cargo.toml
	cargo-deb --no-build --manifest-path=../usbsas-analyzer-server/Cargo.toml

$(PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb:
	cargo build --release --manifest-path=../usbsas-hid/hid-user/Cargo.toml
	cargo build --release --manifest-path=../usbsas-hid/hid-dealer/Cargo.toml
	cargo-deb --no-build --manifest-path=../usbsas-hid/hid-dealer/Cargo.toml

../live-iso/usbsas-$(ISO_VER)-amd64.hybrid.iso: pkgs
	make -C ../live-iso

clean:
	make -C ../live-iso clean

distclean: clean
	make -C ../live-iso distclean
	cargo clean --manifest-path=../Cargo.toml
