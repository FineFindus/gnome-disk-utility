use std::os::fd::AsRawFd;

/// Device size in bytes of the block device from `fd`.
///
/// # Errors
///
/// Returns an error, if the given file descriptor is not for a block device.
pub fn device_size(fd: &impl AsRawFd) -> std::io::Result<u64> {
    // Defined in Linux/fs.h
    const BLKGETSIZE64_CODE: u8 = 0x12;
    const BLKGETSIZE64_SEQ: u8 = 114;
    nix::ioctl_read!(blkgetsize64, BLKGETSIZE64_CODE, BLKGETSIZE64_SEQ, u64);

    let mut block_device_size = 0;
    if unsafe { blkgetsize64(fd.as_raw_fd(), &mut block_device_size) } != Ok(0) {
        log::error!("Error determining size of device");
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }

    Ok(block_device_size)
}

