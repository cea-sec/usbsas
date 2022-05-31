#ifndef UNIX_IO_H
#define UNIX_IO_H

extern s64 ntfs_dev_read(struct ntfs_device *dev, const void* buf, u64 count);
extern s64 ntfs_dev_write(struct ntfs_device *dev, const void* buf, u64 count);
extern s64 ntfs_dev_lseek(struct ntfs_device *dev, s64 offset, int whence);

#endif /* UNIX_IO_H */
