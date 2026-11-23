ISO_VER=0.3.3

DST_PKG_DIR=config/includes.chroot
SRC_PKG_DIR=../../target/debian

export USBSAS_ISO_VER=$(ISO_VER)

SRV_VER := $(shell grep '^version = ' ../../usbsas-usbsas/Cargo.toml | cut -d'"' -f2)-1
ASRV_VER := $(shell grep '^version = ' ../../usbsas-analyzer-server/Cargo.toml | cut -d'"' -f2)-1
HID_VER := $(shell grep '^version = ' ../../usbsas-hid/Cargo.toml | cut -d'"' -f2)-1
KIOSK_VER := $(shell grep '^version = ' ../../usbsas-client/Cargo.toml | cut -d'"' -f2)-1

SU=sudo

.PHONY: all pkgs clean distclean

all: usbsas-$(ISO_VER)-amd64.hybrid.iso

PKGS = $(DST_PKG_DIR)/usbsas-kiosk_$(KIOSK_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-server_$(SRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-analyzer-server_$(ASRV_VER)_amd64.deb \
	$(DST_PKG_DIR)/usbsas-hid_$(HID_VER)_amd64.deb

# Remove packages of previous versions, the chroot hook expects only one
pkgs: $(PKGS)
	@for f in $(DST_PKG_DIR)/*.deb; do \
		case " $(PKGS) " in *" $$f "*) ;; *) rm -fv "$$f" ;; esac; \
	done

$(DST_PKG_DIR)/%.deb: $(SRC_PKG_DIR)/%.deb
	cp $< $@

# Start from a clean tree (the package cache is kept), otherwise stage files
# of a previous build could make lb skip the chroot stages
usbsas-$(ISO_VER)-amd64.hybrid.iso: pkgs
	$(SU) lb clean
	lb config
	$(SU) lb build

clean:
	$(SU) lb clean
	rm -f $(DST_PKG_DIR)/*.deb

distclean: clean
	$(SU) rm -rf cache
