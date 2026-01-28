#ifndef _NTFS_MKNTFS_H
#define _NTFS_MKNTFS_H

/**
 * struct mkntfs_options
 */
struct mkntfs_options {
	char *dev_name;			/* Name of the device, or file, to use */
	BOOL enable_compression;	/* -C, enables compression of all files on the volume by default. */
	BOOL quick_format;		/* -f or -Q, fast format, don't zero the volume first. */
	BOOL force;			/* -F, force fs creation. */
	long heads;			/* -H, number of heads on device */
	BOOL disable_indexing;		/* -I, disables indexing of file contents on the volume by default. */
	BOOL no_action;			/* -n, do not write to device, only display what would be done. */
	long long part_start_sect;	/* -p, start sector of partition on parent device */
	long sector_size;		/* -s, in bytes, power of 2, default is 512 bytes. */
	long sectors_per_track;		/* -S, number of sectors per track on device */
	BOOL use_epoch_time;		/* -T, fake the time to be 00:00:00 UTC, Jan 1, 1970. */
	long mft_zone_multiplier;	/* -z, value from 1 to 4. Default is 1. */
	long long num_sectors;		/* size of device in sectors */
	long cluster_size;		/* -c, format with this cluster-size */
	BOOL with_uuid;			/* -U, request setting an uuid */
	char *label;			/* -L, volume label */
};

extern struct mkntfs_options opts;

extern int mkntfs(struct mkntfs_options *opts2, void *priv_data);

#endif /* defined _NTFS_MKNTFS_H */
