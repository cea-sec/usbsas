/**
 * unix_io.c - Unix style disk io functions. Originated from the Linux-NTFS project.
 *
 * Copyright (c) 2000-2006 Anton Altaparmakov
 *
 * This program/include file is free software; you can redistribute it and/or
 * modify it under the terms of the GNU General Public License as published
 * by the Free Software Foundation; either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program/include file is distributed in the hope that it will be
 * useful, but WITHOUT ANY WARRANTY; without even the implied warranty
 * of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program (in the main directory of the NTFS-3G
 * distribution in the file COPYING); if not, write to the Free Software
 * Foundation,Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
 */

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#ifdef HAVE_UNISTD_H
#include <unistd.h>
#endif
#ifdef HAVE_STDLIB_H
#include <stdlib.h>
#endif
#ifdef HAVE_STRING_H
#include <string.h>
#endif
#ifdef HAVE_ERRNO_H
#include <errno.h>
#endif
#ifdef HAVE_STDIO_H
#include <stdio.h>
#endif
#ifdef HAVE_SYS_TYPES_H
#include <sys/types.h>
#endif
#ifdef HAVE_SYS_STAT_H
#include <sys/stat.h>
#endif
#ifdef HAVE_FCNTL_H
#include <fcntl.h>
#endif
#ifdef HAVE_SYS_IOCTL_H
#include <sys/ioctl.h>
#endif
#ifdef HAVE_LINUX_FD_H
#include <linux/fd.h>
#endif
#ifdef HAVE_LINUX_FS_H
#include <linux/fs.h>
#endif

#include "types.h"
#include "mst.h"
#include "debug.h"
#include "device.h"
#include "logging.h"
#include "misc.h"
#include "unix_io.h"

#define DEV_FD(dev)	(*(int *)dev->d_private)

/* Define to nothing if not present on this system. */
#ifndef O_EXCL
#	define O_EXCL 0
#endif

/**
 * fsync replacement which makes every effort to try to get the data down to
 * disk, using different means for different operating systems. Specifically,
 * it issues the proper fcntl for Mac OS X or does fsync where it is available
 * or as a last resort calls the fsync function. Information on this problem
 * was retrieved from:
 *   http://mirror.linux.org.au/pub/linux.conf.au/2007/video/talks/278.pdf
 */
/*
static int ntfs_fsync(int fildes)
{
}
*/

/**
 * ntfs_device_unix_io_open - Open a device and lock it exclusively
 * @dev:
 * @flags:
 *
 * Description...
 *
 * Returns:
 */
static int ntfs_device_unix_io_open(struct ntfs_device *dev, int flags)
{
	return 0;
}

/**
 * ntfs_device_unix_io_close - Close the device, releasing the lock
 * @dev:
 *
 * Description...
 *
 * Returns:
 */
static int ntfs_device_unix_io_close(struct ntfs_device *dev)
{
	return 0;
}

/**
 * ntfs_device_unix_io_seek - Seek to a place on the device
 * @dev:
 * @offset:
 * @whence:
 *
 * Description...
 *
 * Returns:
 */
static s64 ntfs_device_unix_io_seek(struct ntfs_device *dev, s64 offset,
		int whence)
{
	return ntfs_dev_lseek(dev, offset, whence);
}

/**
 * ntfs_device_unix_io_read - Read from the device, from the current location
 * @dev:
 * @buf:
 * @count:
 *
 * Description...
 *
 * Returns:
 */
static s64 ntfs_device_unix_io_read(struct ntfs_device *dev, void *buf,
		s64 count)
{
	return -1;
}

/**
 * ntfs_device_unix_io_write - Write to the device, at the current location
 * @dev:
 * @buf:
 * @count:
 *
 * Description...
 *
 * Returns:
 */
static s64 ntfs_device_unix_io_write(struct ntfs_device *dev, const void *buf,
		s64 count)
{
	return ntfs_dev_write(dev, buf, count);
}

/**
 * ntfs_device_unix_io_pread - Perform a positioned read from the device
 * @dev:
 * @buf:
 * @count:
 * @offset:
 *
 * Description...
 *
 * Returns:
 */
static s64 ntfs_device_unix_io_pread(struct ntfs_device *dev, void *buf,
		s64 count, s64 offset)
{
	if (ntfs_dev_lseek(dev, offset, 0) != offset) {
		return -1;
	}
	return ntfs_dev_read(dev, buf, count);
}

/**
 * ntfs_device_unix_io_pwrite - Perform a positioned write to the device
 * @dev:
 * @buf:
 * @count:
 * @offset:
 *
 * Description...
 *
 * Returns:
 */
static s64 ntfs_device_unix_io_pwrite(struct ntfs_device *dev, const void *buf,
		s64 count, s64 offset)
{
	if (ntfs_dev_lseek(dev, offset, 0) != offset ) {
		return -1;;
	}
	return ntfs_dev_write(dev, buf, count);
}

/**
 * ntfs_device_unix_io_sync - Flush any buffered changes to the device
 * @dev:
 *
 * Description...
 *
 * Returns:
 */
static int ntfs_device_unix_io_sync(struct ntfs_device *dev)
{
	return 0;
}

/**
 * ntfs_device_unix_io_stat - Get information about the device
 * @dev:
 * @buf:
 *
 * Description...
 *
 * Returns:
 */
static int ntfs_device_unix_io_stat(struct ntfs_device *dev, struct stat *buf)
{
	return -1;
}

/**
 * ntfs_device_unix_io_ioctl - Perform an ioctl on the device
 * @dev:
 * @request:
 * @argp:
 *
 * Description...
 *
 * Returns:
 */
static int ntfs_device_unix_io_ioctl(struct ntfs_device *dev,
		unsigned long request, void *argp)
{
	return 0;
}

/**
 * Device operations for working with unix style devices and files.
 */
struct ntfs_device_operations ntfs_device_unix_io_ops = {
	.open		= ntfs_device_unix_io_open,
	.close		= ntfs_device_unix_io_close,
	.seek		= ntfs_device_unix_io_seek,
	.read		= ntfs_device_unix_io_read,
	.write		= ntfs_device_unix_io_write,
	.pread		= ntfs_device_unix_io_pread,
	.pwrite		= ntfs_device_unix_io_pwrite,
	.sync		= ntfs_device_unix_io_sync,
	.stat		= ntfs_device_unix_io_stat,
	.ioctl		= ntfs_device_unix_io_ioctl,
};
