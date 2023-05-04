ISO_VER=$(shell grep '^version' ../../Cargo.toml | cut -d'"' -f2)

DST_PKG_DIR=config/includes.chroot
SRC_PKG_DIR=../../target/debian

export USBSAS_ISO_VER=$(ISO_VER)

CORE_VER := $(shell grep '^version = ' ../../usbsas-usbsas/Cargo.toml | cut -d'"' -f2)
SRV_VER := $(shell grep '^version = ' ../../usbsas-server/Cargo.toml | cut -d'"' -f2)
ASRV_VER := $(shell grep '^version = ' ../../usbsas-analyzer-server/Cargo.toml | cut -d'"' -f2)
HID_VER := $(shell grep '^version = ' ../../usbsas-hid/hid-dealer/Cargo.toml | cut -d'"' -f2)
KIOSK_VER := $(shell grep 'Version' ../kiosk/usbsas-kiosk/DEBIAN/control | cut -d' ' -f2)

SU=sudo

all: usbsas-$(ISO_VER)-amd64.hybrid.iso

pkgs: $(DST_PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-core_$(CORE_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb

$(DST_PKG_DIR)/%.deb: $(SRC_PKG_DIR)/%.deb
	cp $< $@

usbsas-$(ISO_VER)-amd64.hybrid.iso: pkgs
	lb config
	$(SU) lb build

clean:
	$(SU) lb clean

distclean: clean
	$(SU) rm -rf cache
	rm $(DST_PKG_DIR)/*.deb
