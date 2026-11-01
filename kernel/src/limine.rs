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