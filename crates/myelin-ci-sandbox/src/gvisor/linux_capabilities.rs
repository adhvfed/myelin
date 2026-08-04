use std::io;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
pub(super) const CAP_DAC_READ_SEARCH_NUMBER: u32 = 2;
const PR_CAP_AMBIENT: libc::c_int = 47;
const PR_CAP_AMBIENT_IS_SET: libc::c_ulong = 1;
const PR_CAP_AMBIENT_LOWER: libc::c_ulong = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCapabilityHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct LinuxCapabilityData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

pub(super) fn current_thread_capabilities() -> io::Result<[LinuxCapabilityData; 2]> {
    let mut header = LinuxCapabilityHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [LinuxCapabilityData::default(); 2];
    let result = unsafe {
        libc::syscall(
            libc::SYS_capget,
            &mut header as *mut LinuxCapabilityHeader,
            data.as_mut_ptr(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(data)
    }
}

pub(super) fn set_current_thread_capabilities(data: &[LinuxCapabilityData; 2]) -> io::Result<()> {
    let mut header = LinuxCapabilityHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let result = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &mut header as *mut LinuxCapabilityHeader,
            data.as_ptr(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn capability_mask(capability: u32) -> (usize, u32) {
    ((capability / 32) as usize, 1u32 << (capability % 32))
}

pub(super) fn capability_is_permitted(data: &[LinuxCapabilityData; 2], capability: u32) -> bool {
    let (word, mask) = capability_mask(capability);
    data[word].permitted & mask != 0
}

pub(super) fn capability_is_effective(data: &[LinuxCapabilityData; 2], capability: u32) -> bool {
    let (word, mask) = capability_mask(capability);
    data[word].effective & mask != 0
}

pub(super) fn capability_is_inheritable(data: &[LinuxCapabilityData; 2], capability: u32) -> bool {
    let (word, mask) = capability_mask(capability);
    data[word].inheritable & mask != 0
}

pub(super) fn set_capability_effective(
    data: &mut [LinuxCapabilityData; 2],
    capability: u32,
    effective: bool,
) {
    let (word, mask) = capability_mask(capability);
    if effective {
        data[word].effective |= mask;
    } else {
        data[word].effective &= !mask;
    }
}

fn normalize_capability_sets(
    data: &mut [LinuxCapabilityData; 2],
    capability: u32,
    retain_permitted: bool,
) {
    let (word, mask) = capability_mask(capability);
    data[word].effective &= !mask;
    data[word].inheritable &= !mask;
    if retain_permitted {
        data[word].permitted |= mask;
    } else {
        data[word].permitted &= !mask;
    }
}

pub(super) fn ambient_capability_is_set(capability: u32) -> io::Result<bool> {
    let result = unsafe {
        libc::prctl(
            PR_CAP_AMBIENT,
            PR_CAP_AMBIENT_IS_SET,
            capability as libc::c_ulong,
            0,
            0,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result == 1)
    }
}

pub fn prepare_checkout_host_verification_capability(retain_permitted: bool) -> Result<(), String> {
    let mut capabilities = current_thread_capabilities()
        .map_err(|error| format!("read initial thread capabilities: {error}"))?;
    if retain_permitted && !capability_is_permitted(&capabilities, CAP_DAC_READ_SEARCH_NUMBER) {
        return Err(
            "CAP_DAC_READ_SEARCH is absent from the permitted set; the explicit-userns checkout \
             host verifier requires this read-only DAC bypass"
                .to_string(),
        );
    }
    if ambient_capability_is_set(CAP_DAC_READ_SEARCH_NUMBER)
        .map_err(|error| format!("query ambient CAP_DAC_READ_SEARCH: {error}"))?
    {
        let result = unsafe {
            libc::prctl(
                PR_CAP_AMBIENT,
                PR_CAP_AMBIENT_LOWER,
                CAP_DAC_READ_SEARCH_NUMBER as libc::c_ulong,
                0,
                0,
            )
        };
        if result < 0 {
            return Err(format!(
                "lower ambient CAP_DAC_READ_SEARCH: {}",
                io::Error::last_os_error()
            ));
        }
    }
    normalize_capability_sets(
        &mut capabilities,
        CAP_DAC_READ_SEARCH_NUMBER,
        retain_permitted,
    );
    set_current_thread_capabilities(&capabilities)
        .map_err(|error| format!("normalize CAP_DAC_READ_SEARCH sets: {error}"))?;

    let verified = current_thread_capabilities()
        .map_err(|error| format!("re-read prepared thread capabilities: {error}"))?;
    if capability_is_permitted(&verified, CAP_DAC_READ_SEARCH_NUMBER) != retain_permitted
        || capability_is_effective(&verified, CAP_DAC_READ_SEARCH_NUMBER)
        || capability_is_inheritable(&verified, CAP_DAC_READ_SEARCH_NUMBER)
        || ambient_capability_is_set(CAP_DAC_READ_SEARCH_NUMBER)
            .map_err(|error| format!("re-query ambient CAP_DAC_READ_SEARCH: {error}"))?
    {
        return Err(if retain_permitted {
            "CAP_DAC_READ_SEARCH did not settle into permitted-only state (effective, \
                 inheritable, and ambient must all be absent)"
                .to_string()
        } else {
            "CAP_DAC_READ_SEARCH was not fully dropped (permitted, effective, inheritable, \
                 and ambient must all be absent)"
                .to_string()
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_startup_normalization_keeps_only_permitted_dac_read_search() {
        let mut capabilities = [LinuxCapabilityData::default(); 2];
        let (word, mask) = capability_mask(CAP_DAC_READ_SEARCH_NUMBER);
        capabilities[word].permitted |= mask;
        capabilities[word].effective |= mask;
        capabilities[word].inheritable |= mask;

        normalize_capability_sets(&mut capabilities, CAP_DAC_READ_SEARCH_NUMBER, true);

        assert!(capability_is_permitted(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
        assert!(!capability_is_effective(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
        assert!(!capability_is_inheritable(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
    }

    #[test]
    fn non_enabled_startup_normalization_drops_dac_read_search_entirely() {
        let mut capabilities = [LinuxCapabilityData::default(); 2];
        let (word, mask) = capability_mask(CAP_DAC_READ_SEARCH_NUMBER);
        capabilities[word].permitted |= mask;
        capabilities[word].effective |= mask;
        capabilities[word].inheritable |= mask;

        normalize_capability_sets(&mut capabilities, CAP_DAC_READ_SEARCH_NUMBER, false);

        assert!(!capability_is_permitted(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
        assert!(!capability_is_effective(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
        assert!(!capability_is_inheritable(
            &capabilities,
            CAP_DAC_READ_SEARCH_NUMBER
        ));
    }
}
