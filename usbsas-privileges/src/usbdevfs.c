#include <inttypes.h>
#include <sys/ioctl.h>
#include <linux/usbdevice_fs.h>

uint64_t usbdevfs_submiturb() {
    return USBDEVFS_SUBMITURB;
}

uint64_t usbdevfs_reapurbndelay() {
    return USBDEVFS_REAPURBNDELAY;
}

uint64_t usbdevfs_releaseinterface() {
    return USBDEVFS_RELEASEINTERFACE;
}

uint64_t usbdevfs_ioctl() {
    return USBDEVFS_IOCTL;
}

uint64_t usbdevfs_discardurb() {
    return USBDEVFS_DISCARDURB;
}

uint64_t usbdevfs_get_capabilities() {
    return USBDEVFS_GET_CAPABILITIES;
}

uint64_t usbdevfs_disconnect_claim() {
    return USBDEVFS_DISCONNECT_CLAIM;
}

uint64_t usbdevfs_reset() {
    return USBDEVFS_RESET;
}
