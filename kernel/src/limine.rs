#[repr(C)]
pub struct LimineMemmapEntry {
    pub base: u64,
    pub length: u64,
    pub entry_type: u64,
}

#[repr(C)]
pub struct LimineMemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *mut *mut LimineMemmapEntry,
}

#[repr(C)]
pub struct LimineMemmapRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut LimineMemmapResponse,
}

pub const LIMINE_MEMMAP_USABLE: u64 = 0;
pub const LIMINE_MEMMAP_RESERVED: u64 = 1;
pub const LIMINE_MEMMAP_ACPI_RECLAIMABLE: u64 = 2;
pub const LIMINE_MEMMAP_ACPI_NVS: u64 = 3;
pub const LIMINE_MEMMAP_BAD_MEMORY: u64 = 4;
pub const LIMINE_MEMMAP_BOOTLOADER_RECLAIMABLE: u64 = 5;
pub const LIMINE_MEMMAP_EXECUTABLE_AND_MODULES: u64 = 6;
pub const LIMINE_MEMMAP_FRAMEBUFFER: u64 = 7;

// Limine memory-map request
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static mut LIMINE_MEMMAP_REQUEST: LimineMemmapRequest =
    LimineMemmapRequest {
        id: [
            0xc7b1dd30df4c8b88,
            0x0a82e883a194f07b,
            0x67cf3d9d378a806f,
            0xe304acdfc50c3c62,
        ],
        revision: 0,
        response: core::ptr::null_mut(),
    };

#[repr(C)]
pub struct LimineHhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct LimineHhdmRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut LimineHhdmResponse,
}

// Limine HHDM request
#[used]
#[unsafe(link_section = ".limine_requests")]
pub static mut LIMINE_HHDM_REQUEST: LimineHhdmRequest =
    LimineHhdmRequest {
        id: [
            0xc7b1dd30df4c8b88,
            0x0a82e883a194f07b,
            0x48dcf1cb8ad2b852,
            0x63984e959a98244b,
        ],
        revision: 0,
        response: core::ptr::null_mut(),
    };


// Request section delimiters
#[used]
#[unsafe(link_section = ".limine_requests_start")]
static LIMINE_REQUESTS_START: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

#[used]
#[unsafe(link_section = ".limine_requests")]
static mut LIMINE_BASE_REVISION: [u64; 3] = [
    0xf9562b2d5c95a6c8,
    0x6a7b384944536bdc,
    3,
];

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static LIMINE_REQUESTS_END: [u64; 2] = [
    0xadc0e0531bb10d03,
    0x9572709f31764c62,
];


pub fn memmap_response_ptr() -> *mut LimineMemmapResponse {
    unsafe {
        core::ptr::addr_of!(LIMINE_MEMMAP_REQUEST.response)
            .read_volatile()
    }
}

pub fn memmap_response_info(
) -> Option<(u64, u64, *mut *mut LimineMemmapEntry)> {
    let response = memmap_response_ptr();

    if response.is_null() {
        return None;
    }

    unsafe {
        let revision =
            core::ptr::addr_of!((*response).revision).read_volatile();

        let entry_count =
            core::ptr::addr_of!((*response).entry_count).read_volatile();

        let entries =
            core::ptr::addr_of!((*response).entries).read_volatile();

        if entries.is_null() || entry_count == 0 {
            return None;
        }

        Some((revision, entry_count, entries))
    }
}

pub fn memmap_entry_at(
    index: u64,
) -> Option<(u64, u64, u64)> {
    let response = memmap_response_ptr();

    if response.is_null() {
        return None;
    }

    unsafe {
        let count =
            core::ptr::addr_of!((*response).entry_count).read_volatile();

        let entries =
            core::ptr::addr_of!((*response).entries).read_volatile();

        if index >= count || index >= 4096 || entries.is_null() {
            return None;
        }

       let entry_ptr = entries.add(index as usize).read_volatile();

        if entry_ptr.is_null() {
            return None;
        }

        let base =
            core::ptr::addr_of!((*entry_ptr).base).read_volatile();

        let length =
            core::ptr::addr_of!((*entry_ptr).length).read_volatile();

        let entry_type =
            core::ptr::addr_of!((*entry_ptr).entry_type).read_volatile();

        if length == 0 || base.checked_add(length).is_none() {
            return None;
        }

        Some((base, length, entry_type))
    }
}


#[derive(Debug, PartialEq, Eq)]
pub enum MemmapValidationError {
    MissingResponse,
    TooManyEntries,
    InvalidEntry(u64),
    UnsortedOrOverlapping(u64),
}

pub fn validate_memmap() -> Result<u64, MemmapValidationError> {
    let (_, entry_count, _) =
        memmap_response_info().ok_or(
            MemmapValidationError::MissingResponse
        )?;

    if entry_count > 4096 {
        return Err(MemmapValidationError::TooManyEntries);
    }

    let mut previous_end: Option<u64> = None;
    let mut unknown_type_count = 0;

    let mut index = 0;

    while index < entry_count {
        let (base, length, entry_type) =
            memmap_entry_at(index).ok_or(
                MemmapValidationError::InvalidEntry(index)
            )?;

        let end = base.checked_add(length).ok_or(
            MemmapValidationError::InvalidEntry(index)
        )?;

        if length == 0 {
            return Err(MemmapValidationError::InvalidEntry(index));
        }

        if let Some(previous) = previous_end {
            if base < previous {
                return Err(
                    MemmapValidationError::UnsortedOrOverlapping(index)
                );
            }
        }

        match entry_type {
            LIMINE_MEMMAP_USABLE
            | LIMINE_MEMMAP_RESERVED
            | LIMINE_MEMMAP_ACPI_RECLAIMABLE
            | LIMINE_MEMMAP_ACPI_NVS
            | LIMINE_MEMMAP_BAD_MEMORY
            | LIMINE_MEMMAP_BOOTLOADER_RECLAIMABLE
            | LIMINE_MEMMAP_EXECUTABLE_AND_MODULES
            | LIMINE_MEMMAP_FRAMEBUFFER => {}

            _ => {
                unknown_type_count += 1;
            }
        }

        previous_end = Some(end);
        index += 1;
    }

    Ok(unknown_type_count)
}


pub struct MemmapStats {
    pub usable_bytes: u64,
    pub usable_region_count: u64,
    pub largest_usable_region: u64,
    pub bootloader_reclaimable_bytes: u64,
    pub bootloader_reclaimable_region_count: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MemmapStatsError {
    InvalidMemoryMap,
    InvalidEntry(u64),
    ArithmeticOverflow,
}

pub fn calculate_memmap_stats(
) -> Result<MemmapStats, MemmapStatsError> {
    // Never calculate statistics from an invalid memory map.
    validate_memmap().map_err(
        |_| MemmapStatsError::InvalidMemoryMap
    )?;

    let (_, entry_count, _) =
        memmap_response_info().ok_or(
            MemmapStatsError::InvalidMemoryMap
        )?;

    let mut stats = MemmapStats {
        usable_bytes: 0,
        usable_region_count: 0,
        largest_usable_region: 0,
        bootloader_reclaimable_bytes: 0,
        bootloader_reclaimable_region_count: 0,
    };

    let mut index = 0;

    while index < entry_count {
        let (_, length, entry_type) =
            memmap_entry_at(index).ok_or(
                MemmapStatsError::InvalidEntry(index)
            )?;

        match entry_type {
            LIMINE_MEMMAP_USABLE => {
                stats.usable_bytes = stats.usable_bytes
                    .checked_add(length)
                    .ok_or(MemmapStatsError::ArithmeticOverflow)?;

                stats.usable_region_count += 1;

                if length > stats.largest_usable_region {
                    stats.largest_usable_region = length;
                }
            }

            LIMINE_MEMMAP_BOOTLOADER_RECLAIMABLE => {
                stats.bootloader_reclaimable_bytes =
                    stats.bootloader_reclaimable_bytes
                    .checked_add(length)
                    .ok_or(MemmapStatsError::ArithmeticOverflow)?;

                stats.bootloader_reclaimable_region_count += 1;
            }

            _ => {}
        }

        index += 1;
    }

    Ok(stats)
}


pub fn hhdm_response_info() -> Option<(u64, u64)> {
    let response = unsafe {
        core::ptr::addr_of!(LIMINE_HHDM_REQUEST.response)
            .read_volatile()
    };

    if response.is_null() {
        return None;
    }

    unsafe {
        let revision =
            core::ptr::addr_of!((*response).revision).read_volatile();

        let offset =
            core::ptr::addr_of!((*response).offset).read_volatile();

        if offset == 0 || offset % 4096 != 0 {
            return None;
        }

        Some((revision, offset))
    }
}


pub fn physical_to_virtual(
    physical_address: u64,
) -> Option<*mut u8> {
    let (_, hhdm_offset) = hhdm_response_info()?;

    // Require the physical address to belong to a memory-map entry.
    let (_, entry_count, _) = memmap_response_info()?;

    let mut mapped_range = false;
    let mut index = 0;

    while index < entry_count {
        let (base, length, _) = memmap_entry_at(index)?;

        let end = base.checked_add(length)?;

        if physical_address >= base && physical_address < end {
            mapped_range = true;
            break;
        }

        index += 1;
    }

    if !mapped_range {
        return None;
    }

    // Calculate the HHDM virtual address.
    let virtual_address =
        hhdm_offset.checked_add(physical_address)?;

    // Verify x86-64 canonical addressing.
    let upper = virtual_address >> 48;
    let sign_bit = (virtual_address >> 47) & 1;

    let canonical = if sign_bit == 0 {
        upper == 0
    } else {
        upper == 0xFFFF
    };

    if !canonical {
        return None;
    }

    Some(virtual_address as *mut u8)
}