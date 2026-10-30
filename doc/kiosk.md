# usbsas kiosk deployment

This documentation describes how to deploy a usbsas kiosk (or "sheep dip")
station: a dedicated machine that boots directly to the usbsas interface, on
which users can't do anything else than transferring files between USB devices.

usbsas only protects the station against what comes through the USB ports
(see the [threat model](./architecture.md#threatmodel)). The rest of the
system has to be hardened as well.

* [Overview](#overview)
* [Hardware and firmware](#hardware)
* [Operating system installation](#os)
* [usbsas installation](#installation)
* [Kernel modules](#modules)
* [usbsas configuration](#configuration)
* [System hardening](#hardening)
    * [Secure boot](#secureboot)
    * [Disk encryption with TPM unlock](#tpm)
    * [Kernel](#kernel)
    * [Accounts and access](#accounts)
    * [Network](#network)
    * [Updates and logs](#updates)

## <a name="overview">Overview</a>

A secure station combines several layers, each one covered below:

| Layer | Goal | Means |
|-------|------|-------|
| Hardware and firmware | Nobody can boot something else or tamper with the machine | Firmware password, boot order locked, unused interfaces disabled, physical protection |
| Boot chain | Only the expected system boots | UEFI Secure Boot, locked bootloader and kernel command line |
| Storage | The disk can't be read or modified offline | Full disk encryption, unlocked automatically by the TPM only if the boot chain is the expected one |
| Kernel | Devices are handled by usbsas, not by the kernel | Mass storage, file system and HID modules disabled, kernel hardening |
| usbsas | Untrusted devices are handled in sandboxed processes | usbsas packages, configuration |
| System | Users are confined to the interface | Kiosk session, no local login, restricted remote administration, firewall |
| Operations | The station stays up to date and transfers are traceable | Security updates, central logs, reports |

## <a name="hardware">Hardware and firmware</a>

- Use a **dedicated machine**, used for nothing else.
- Set a **firmware (UEFI) administrator password**, so the settings below can't
  be changed.
- **Only boot from the internal disk**: disable boot from USB, optical drives
  and the network (PXE).
- **Disable what isn't needed**: Wi-Fi, Bluetooth, camera, microphone, memory
  card readers, unused USB controllers and ports.
- **Protect against DMA attacks**: disable Thunderbolt / external PCIe if not
  needed, enable the IOMMU (VT-d / AMD-Vi) and the kernel DMA protection.
- Enable the **TPM 2.0** (needed for [automatic disk
  unlock](#tpm)) and **Secure Boot** (see [below](#secureboot)).
- Keep the **firmware up to date**.
- **Physically protect the machine**: lock the case (and enable chassis
  intrusion detection if available), make the internal ports and buttons
  unreachable, only expose the USB ports users need. Label them ("source",
  "destination"): usbsas can enforce these roles with [USB port
  assignment](./configuration.md#usbport).

## <a name="os">Operating system installation</a>

Install a minimal **Debian 13 (trixie)**:

- no desktop environment and no other services than the ones needed (only "SSH
  server" if the station is administered remotely, see [Accounts and
  access](#accounts)),
- with **encrypted partitions**, the passphrase chosen here becomes the
  recovery key once the [TPM unlock](#tpm) is set up,
- with a **UEFI** installation, needed for Secure Boot.

The working files of usbsas (`/var/lib/usbsas` by default) contain the users'
files during transfers: keep them on the encrypted disk, or in memory (tmpfs, if
the station has enough memory: about twice the size of the largest transfer).

## <a name="installation">usbsas installation</a>

### <a name="packages">Packages</a>

Debian packages (amd64) can be downloaded from the [release
page](https://github.com/cea-sec/usbsas/releases/latest) or
[built](./build.md#packages).

#### <a name="debserver">usbsas-server</a>

The usbsas server and its processes (in `/usr/libexec`), the configuration
file (`/etc/usbsas/config.toml`) and the `usbsas-server` systemd service.

When installed, it creates the `usbsas` user, which runs the server, installs a
udev rule giving this user the ownership of USB mass storage devices, and a
modprobe configuration preventing the `usb_storage` and `uas` modules from
loading.

#### <a name="debkiosk">usbsas-kiosk</a>

The graphical interface and the `usbsas-client` systemd service, which starts
it in full screen in the [cage](https://github.com/cage-kiosk/cage) kiosk
compositor, on tty2. It depends on `usbsas-server`.

When installed, it creates the `usbsas-client` user, which runs the interface,
and masks the login prompt (getty) of tty2.

#### <a name="debhid">usbsas-hid</a>

A minimal HID driver running in user space, which replaces the kernel HID
drivers to protect the station against "BadUSB" devices: it only handles
pointer motion and left click (mice and touch screens), **keyboards are
ignored**. Left click is all the interface needs. It depends on
`usbsas-kiosk`.

When installed, it creates the `usbsas-hid` user and installs:

- the `hid-dealer` systemd service, which starts a driver process for each HID
  device plugged (on an allowed port, if `ports_hid` is configured),
- a udev rule giving the `usbsas-hid` user the ownership of HID devices and of
  `/dev/uinput`, and a `modules-load.d` file loading the `uinput` module,
- a modprobe configuration preventing the kernel HID, mouse and keyboard
  drivers from loading (`hid`, `usbhid`, `hid_generic`, `hid_multitouch`,
  `usbmouse`, `mousedev`, `psmouse`, `atkbd`, `i8042`).

Installing it is recommended. Without it, the kernel drivers handle mice and
keyboards, so a malicious device pretending to be a keyboard could type
commands.

#### <a name="debanalyzer">usbsas-analyzer-server</a>

The **demo** analyzer server (ClamAV), see
[configuration](./configuration.md#remote). It isn't meant for production: a
production station should use a dedicated analysis platform, on another
machine.

#### usbsas-tools

The [administration tools](./configuration.md#tools). They aren't needed on a
production station.

### Installation

**Warning**: once these packages are installed and the services enabled, the
station boots to the usbsas interface and, with `usbsas-hid`, keyboards no
longer work. Make sure you have another way to administer it (SSH for
instance, see [Accounts and access](#accounts)) before rebooting.

```shell
$ sudo apt install ./usbsas-server_X.Y.Z-1_amd64.deb \
                   ./usbsas-kiosk_X.Y.Z-1_amd64.deb \
                   ./usbsas-hid_X.Y.Z-1_amd64.deb
```

Configure usbsas (see [below](#configuration)), then enable the services and
reboot:

```shell
$ sudo systemctl enable usbsas-server.service usbsas-client.service hid-dealer.service
$ sudo reboot
```

The station boots to the usbsas interface.

## <a name="modules">Kernel modules</a>

The kernel must leave USB devices to usbsas. The packages already prevent these
modules from loading:

- `usb_storage` and `uas` (`usbsas-server`): mass storage,
- the HID, mouse and keyboard drivers (`usbsas-hid`).

The packages update the initramfs so that these modules can't be loaded early
at boot either.

The station should also refuse every other kind of USB device a malicious
device could pretend to be. Prevent the modules of USB classes the station
doesn't need from loading, the same way (`install <module> /bin/false` in a
file of `/etc/modprobe.d/`), for instance:

- network: `usbnet`, `cdc_ether`, `cdc_ncm`, `rndis_host`, `r8152`, `ax88179_178a`,
- serial and modems: `cdc_acm`, `usbserial` and its drivers,
- audio and video: `snd_usb_audio`, `uvcvideo`,
- Bluetooth: `btusb`,
- printers: `usblp`,
- file systems the station doesn't use: `vfat`, `exfat`, `ntfs3`, `hfsplus`,
  `udf`, `isofs`... (usbsas never mounts file systems).

A **custom kernel** built without these drivers, or without loadable module
support, is even better. Otherwise, once the system has booted, module loading
can be disabled altogether (see [Kernel](#kernel)).

`lsmod` shows the loaded modules: plug devices of every kind on the station and
check that none of these drivers gets loaded.

## <a name="configuration">usbsas configuration</a>

Edit `/etc/usbsas/config.toml`, the options are described in the
[configuration documentation](./configuration.md). For a kiosk station, pay
attention to:

- **Analyzer**: configure a real analysis platform (`[analyzer]`), not the demo
  server, and enable the analysis for every destination.
- **Remote servers**: use HTTPS, and Kerberos authentication if available
  (`krb_service_name`).
- **USB ports**: restrict sources, destinations and HID devices to dedicated
  ports (`[usb_port_accesses]`, use `usbsas-devmonitor` to find their paths).
- **User identification**: replace the placeholder identifier with one that
  identifies users (see [User identification](./configuration.md#identifier)).
- **Reports**: write them on the destination and locally (`[report]`) for
  traceability.
- **Filters** and **maximum depth**: adjust them to your needs.
- **Commands**: the `[command]` and `[post_copy]` commands aren't sandboxed,
  only use trusted programs that handle untrusted files safely.

For a station whose kernel is known to support landlock (check that `landlock`
is listed in `/sys/kernel/security/lsm`), consider building usbsas with the `usbsas-sandbox/landlock-enforce` feature,
so that a process fails instead of running without its landlock rules (see
[build options](./build.md#options)).

## <a name="hardening">System hardening</a>

This section describes what should be done, more than how: details depend on the
hardware and on your  policies. Hardening guides like the [ANSSI configuration
recommendations for GNU/Linux
systems](https://messervices.cyber.gouv.fr/guides/recommandations-de-securite-relatives-un-systeme-gnulinux)
or the [Kernel Self Protection Project](https://kspp.github.io/) give more
details.

### <a name="secureboot">Secure boot</a>

UEFI Secure Boot ensures that only signed bootloaders and kernels are
executed, so the station can't be made to boot a modified system.

- Enable Secure Boot in the firmware. Enroll your own keys (and remove the
  default ones) and sign the boot chain yourself. A custom kernel must be signed
  (with a key enrolled in the firmware).
- With Secure Boot, the Debian kernel enables its **lockdown** mode, which,
  among other things, prevents root from modifying the running kernel.
- **Lock the kernel command line**: anyone able to edit it at boot (e.g.
  `init=/bin/sh`) bypasses the rest of the hardening. Set a GRUB password and
  disable its menu and editor, or better, boot a signed **unified kernel image**
  (UKI) embedding the kernel, the initramfs and the command line.

### <a name="tpm">Disk encryption with TPM unlock</a>

Setup automatic TPM2 LUKS decryption (with at least PCR 7), keep the passphrase
as recovery key.

### <a name="kernel">Kernel</a>

- Keep the kernel up to date: its USB host controller drivers and USB core
  still handle every device plugged (see the [threat
  model](./architecture.md#threatmodel)).
- **Disable module loading** after boot (`kernel.modules_disabled = 1` with
  sysctl, once every needed module is loaded, including `uinput`), or use a
  kernel without module support.
- Apply the usual hardening settings, for instance: sysctl: `kernel.kptr_restrict = 2`, `kernel.dmesg_restrict = 1`,
    `kernel.unprivileged_bpf_disabled = 1`, `kernel.yama.ptrace_scope = 2`,
    `kernel.kexec_load_disabled = 1`, `kernel.sysrq = 0`,

### <a name="accounts">Accounts and access</a>

- Users only have access to the usbsas interface: no other session, no login
  prompt on the consoles (mask the `getty@` services of every tty, the
  `usbsas-kiosk` package already masks tty2's), keyboards disabled by
  `usbsas-hid`.
- Each usbsas component runs as its own unprivileged user (`usbsas`,
  `usbsas-client`, `usbsas-hid`); don't add them to other groups nor give them
  sudo rights.
- Lock the root account and restrict administrators: if the station must be
  administered remotely, use SSH with keys only, from the administration
  network only, with no password authentication and no root login. Otherwise,
  remove the SSH server.
- Remove what isn't needed: compilers, debugging tools, unused services.
- Mount `/tmp`, `/var/tmp` and `/dev/shm` with `nodev,nosuid,noexec`. A
  read-only root file system makes persistence even harder.

### <a name="network">Network</a>

If the station has network destinations, a network source or a remote
analyzer, it's connected to a network: limit what it can reach.

- Firewall (nftables): no incoming connection except SSH from the
  administration network; outgoing connections only to the analyzer, the
  remote servers, and the needed infrastructure (DNS, NTP, Kerberos KDC, update
  mirror, log server).
- Use HTTPS with certificates checked against your own certificate authority.
- If the station doesn't need the network, don't connect it.

### <a name="updates">Updates and logs</a>

- Install **security updates** regularly, for Debian and for usbsas, for
  instance with `unattended-upgrades` or your configuration management.
- **Synchronize the clock** (NTP), so that logs and reports have meaningful
  timestamps.
- **Centralize the logs**: forward the journal (with `systemd-journal-upload`,
  rsyslog...) to a log server. At the `info` level, usbsas logs every user
  action and transferred file (see [logs](./configuration.md#services)); the
  `log-json` build feature eases their processing.
- Keep the **transfer reports** (`[report] write_local`) and collect them as
  well.
- Monitor the station: crashes of usbsas processes (killed by seccomp, for
  instance) can be the sign of an attack and are worth looking at.
