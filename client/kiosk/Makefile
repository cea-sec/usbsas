VER := $(shell grep 'Version' usbsas-kiosk/DEBIAN/control | cut -d' ' -f2)
NWJS_VER=0.80.0

PKG=../../target/debian/usbsas-kiosk_$(VER)_amd64.deb

all: $(PKG)

nwjs-v$(NWJS_VER)-linux-x64.tar.gz:
	curl -O "https://dl.nwjs.io/v$(NWJS_VER)/nwjs-v$(NWJS_VER)-linux-x64.tar.gz"

usbsas-kiosk/opt/nwjs/nw: nwjs-v$(NWJS_VER)-linux-x64.tar.gz
	mkdir -p usbsas-kiosk/opt/nwjs
	tar xzf nwjs-v$(NWJS_VER)-linux-x64.tar.gz -C usbsas-kiosk/opt/nwjs --strip-components=1
	chmod -R og+r usbsas-kiosk/opt/nwjs
	find usbsas-kiosk/opt/nwjs -executable | xargs chmod og+x
	find usbsas-kiosk/opt/nwjs -type d | xargs chmod og+rx
	touch usbsas-kiosk/opt/nwjs/nw

usbsas-kiosk/usr/share/usbsas/nwjs:
	mkdir -p usbsas-kiosk/usr/share/usbsas/nwjs
	cp ../nwjs/node-main.js usbsas-kiosk/usr/share/usbsas/nwjs
	cp -P ../nwjs/web usbsas-kiosk/usr/share/usbsas/nwjs
	cp ../nwjs/package.kiosk.json usbsas-kiosk/usr/share/usbsas/nwjs/package.json

usbsas-kiosk/usr/share/usbsas/web:
	mkdir -p usbsas-kiosk/usr/share/usbsas/web
	cp -r ../web usbsas-kiosk/usr/share/usbsas

$(PKG): usbsas-kiosk/usr/share/usbsas/web usbsas-kiosk/usr/share/usbsas/nwjs usbsas-kiosk/opt/nwjs/nw
	mkdir -p ../../target/debian
	dpkg-deb --root-owner-group --build usbsas-kiosk ../../target/debian

clean:
	rm -rf usbsas-kiosk/usr/share/
	rm -rf usbsas-kiosk/opt/nwjs/*

distclean: clean
	rm -f $(PKG)
	rm -f nwjs-v$(NWJS_VER)-linux-x64.tar.gz
