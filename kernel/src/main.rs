
#![no_std]
#![no_main]

mod limine;

use core::arch::asm;
use core::panic::PanicInfo;

const COM1: u16 = 0x3F8;

// Serial port output
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

// Serial port input
unsafe fn inb(port: u16) -> u8 {
    let value: u8;

    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }

    value
}

// Initialize COM1
fn serial_init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

// Write one byte
fn serial_write_byte(byte: u8) {
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {
            core::hint::spin_loop();
        }

        outb(COM1, byte);
    }
}

// Write a string
fn serial_write(message: &str) {
    for byte in message.bytes() {
        serial_write_byte(byte);
    }
}

// Write an unsigned decimal integer
fn serial_write_u64(mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    if value == 0 {
        serial_write("0");
        return;
    }

    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for byte in &buffer[index..] {
        serial_write_byte(*byte);
    }
}

// Detect CPU physical address width
fn physical_address_width() -> Option<u8> {
    use core::arch::x86_64::__cpuid;

    let max_leaf = unsafe {
        __cpuid(0x80000000).eax
    };

    if max_leaf < 0x80000008 {
        return None;
    }

    let result = unsafe {
        __cpuid(0x80000008)
    };

    let width = (result.eax & 0xFF) as u8;

    if !(36..=52).contains(&width) {
        return None;
    }

    Some(width)
}

// Build the physical address mask from the detected CPU width.
fn physical_address_mask(width: u8) -> Option<u64> {
    if !(36..=52).contains(&width) {
        return None;
    }

    Some(((1u64 << width) - 1) & 0x000F_FFFF_FFFF_F000)
}

// Validate a complete physical range represented by a page-table leaf.
fn validate_leaf_physical_range(
    physical_base: u64,
    mapping_size: u64,
    address_width: u8,
) -> bool {
    if mapping_size == 0 || physical_base % mapping_size != 0 {
        return false;
    }

    let range_end = match physical_base.checked_add(mapping_size) {
        Some(end) => end,
        None => return false,
    };

    let physical_limit = match 1u64.checked_shl(address_width as u32) {
        Some(limit) => limit,
        None => return false,
    };

    if range_end > physical_limit {
        return false;
    }

    limine::physical_range_is_classified(
        physical_base,
        mapping_size,
    )
}

fn nx_enabled() -> bool {
    use core::arch::x86_64::__cpuid;

    let max_leaf = __cpuid(0x80000000).eax;

    if max_leaf < 0x80000001 {
        return false;
    }

    let features = __cpuid(0x80000001);

    // Extended feature bit 20: NX support.
    if (features.edx & (1 << 20)) == 0 {
        return false;
    }

    // Read EFER MSR.
    let low: u32;
    let high: u32;

    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0xC0000080u32,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }

    let efer = ((high as u64) << 32) | low as u64;

    // EFER.NXE is bit 11.
    (efer & (1 << 11)) != 0
}

fn supports_2m_pages() -> bool {
    use core::arch::x86_64::__cpuid;

    let features = __cpuid(0x00000001);

    // CPUID.01H:EDX.PSE — bit 3.
    (features.edx & (1 << 3)) != 0
}

fn supports_1g_pages() -> bool {
    use core::arch::x86_64::__cpuid;

    let max_leaf = __cpuid(0x80000000).eax;

    if max_leaf < 0x80000001 {
        return false;
    }

    let features = __cpuid(0x80000001);

    // CPUID.80000001H:EDX.PDPE1GB — bit 26.
    (features.edx & (1 << 26)) != 0
}

fn supports_pat() -> bool {
    use core::arch::x86_64::__cpuid;

    let features = __cpuid(0x00000001);

    // CPUID.01H:EDX.PAT — bit 16.
    (features.edx & (1 << 16)) != 0
}


const IA32_PAT_MSR: u32 = 0x277;

fn read_ia32_pat() -> Option<u64> {
    if !supports_pat() {
        return None;
    }

    let low: u32;
    let high: u32;

    unsafe {
        asm!(
            "rdmsr",
            in("ecx") IA32_PAT_MSR,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }

    Some(((high as u64) << 32) | low as u64)
}

fn pat_memory_type_name(memory_type: u8) -> Option<&'static str> {
    match memory_type {
        0 => Some("UC"),
        1 => Some("WC"),
        2 => None,
        3 => None,
        4 => Some("WT"),
        5 => Some("WP"),
        6 => Some("WB"),
        7 => Some("UC-"),
        _ => None,
    }
}

fn inspect_ia32_pat() -> bool {
    serial_write("[PAT] Starting IA32_PAT inspection...\r\n");

    if !supports_pat() {
        serial_write("[PAT] CPU does not support PAT. Inspection skipped.\r\n");
        return true;
    }

    let pat_value = match read_ia32_pat() {
        Some(value) => value,
        None => {
            serial_write("[PAT] ERROR: Could not read IA32_PAT.\r\n");
            return false;
        }
    };

    let mut index = 0u32;
    let mut valid = true;

    while index < 8 {
        let memory_type =
            ((pat_value >> (index * 8)) & 0xFF) as u8;

        serial_write("[PAT] Entry ");
        serial_write_u64(index as u64);
        serial_write(": ");

        match pat_memory_type_name(memory_type) {
            Some(name) => {
                serial_write(name);
                serial_write(" (");
                serial_write_u64(memory_type as u64);
                serial_write(")\r\n");
            }
            None => {
                serial_write("INVALID RESERVED ENCODING (");
                serial_write_u64(memory_type as u64);
                serial_write(")\r\n");
                valid = false;
            }
        }

        index += 1;
    }

    if valid {
        serial_write("[PAT] IA32_PAT validation PASSED.\r\n");
    } else {
        serial_write("[PAT] ERROR: Invalid memory-type encoding.\r\n");
    }

    valid
}



// Kernel entry point
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial_init();

    serial_write("\r\n[BOOT] Asternel kernel started.\r\n");


        if !inspect_ia32_pat() {
        serial_write("[PAT] Kernel initialization halted.\r\n");

        loop {
            core::hint::spin_loop();
        }
    }

    // Executable-file response
   serial_write("[KERNEL] Checking executable-address response...\r\n");

match limine::executable_address_info() {
    Some((physical_base, virtual_base)) => {
        serial_write("[KERNEL] Executable-address response received.\r\n");

        serial_write("[KERNEL] Physical base: ");
        serial_write_u64(physical_base);
        serial_write("\r\n");

        serial_write("[KERNEL] Virtual base: ");
        serial_write_u64(virtual_base);
        serial_write("\r\n");
    }

    None => {
        serial_write("[KERNEL] Executable-address response unavailable.\r\n");
    }
}

serial_write("[KERNEL] Validating kernel image reservation...\n");

match limine::kernel_image_reservation() {
    Ok((start, end)) => {
        serial_write("[KERNEL] Kernel image reservation validated.\n");
        serial_write("[KERNEL] Reservation start: ");
        serial_write_u64(start);
        serial_write("\n");

        serial_write("[KERNEL] Reservation end: ");
        serial_write_u64(end);
        serial_write("\n");
    }

    Err(error) => {
        serial_write("[KERNEL] Kernel image reservation validation FAILED.\n");

        match error {
            limine::KernelReservationError::MissingExecutableAddress =>
                serial_write("[KERNEL] Missing executable address.\n"),
            limine::KernelReservationError::InvalidAlignment =>
                serial_write("[KERNEL] Invalid address alignment.\n"),
            limine::KernelReservationError::AddressOverflow =>
                serial_write("[KERNEL] Address overflow.\n"),
            limine::KernelReservationError::MissingMemoryMap =>
                serial_write("[KERNEL] Missing memory map.\n"),
            limine::KernelReservationError::InvalidMemoryMap =>
                serial_write("[KERNEL] Invalid memory map.\n"),
            limine::KernelReservationError::KernelRangeNotFound =>
                serial_write("[KERNEL] Kernel range not found.\n"),
            limine::KernelReservationError::KernelRangeNotExecutable =>
                serial_write("[KERNEL] Kernel range is not executable memory.\n"),

            limine::KernelReservationError::InvalidLinkerBounds =>
    serial_write("[KERNEL] Invalid linker image bounds.\n"),

limine::KernelReservationError::VirtualBaseMismatch =>
    serial_write("[KERNEL] Linker and executable virtual bases differ.\n"),
        }
    }
}

    

    // HHDM response
    serial_write("[HHDM] Checking Higher Half Direct Map...\r\n");

    match limine::hhdm_response_info() {
        Some((revision, offset)) => {
            serial_write("[HHDM] Response received.\r\n");

            serial_write("[HHDM] Response revision: ");
            serial_write_u64(revision);
            serial_write("\r\n");

            serial_write("[HHDM] Physical memory offset: ");
            serial_write_u64(offset);
            serial_write("\r\n");

            serial_write("[HHDM] Basic validation PASSED.\r\n");
        }

        None => {
            serial_write("[HHDM] Response missing or invalid.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Physical-to-virtual conversion test
    serial_write("[HHDM] Testing physical-to-virtual conversion...\r\n");

    match limine::physical_to_virtual(0x0010_0000) {
        Some(address) => {
            serial_write("[HHDM] Physical address: 1048576\r\n");

            serial_write("[HHDM] Converted virtual address: ");
            serial_write_u64(address as u64);
            serial_write("\r\n");

            serial_write("[HHDM] Conversion PASSED.\r\n");
        }

        None => {
            serial_write("[HHDM] Conversion FAILED.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Overflow rejection test
    if limine::physical_to_virtual(u64::MAX).is_none() {
        serial_write("[HHDM] Overflow rejection PASSED.\r\n");
    } else {
        serial_write("[HHDM] Overflow rejection FAILED.\r\n");

        loop {
            core::hint::spin_loop();
        }
    }

    // Memory-map boundary rejection test
    if limine::physical_to_virtual(0xFFFFFFFFFFFFF000).is_none() {
        serial_write("[HHDM] Out-of-range rejection PASSED.\r\n");
    } else {
        serial_write("[HHDM] Out-of-range rejection FAILED.\r\n");

        loop {
            core::hint::spin_loop();
        }
    }

    // Memory-map validation
    serial_write("[MEM] Starting memory-map validation...\r\n");

    match limine::validate_memmap() {
        Ok(unknown_count) => {
            serial_write("[MEM] Validation PASSED.\r\n");

            serial_write("[MEM] Unknown entry types: ");
            serial_write_u64(unknown_count);
            serial_write("\r\n");
        }

        Err(error) => {
            serial_write("[MEM] Validation FAILED. Error code: ");

            let code = match error {
                limine::MemmapValidationError::MissingResponse => 1,
                limine::MemmapValidationError::TooManyEntries => 2,
                limine::MemmapValidationError::InvalidEntry(_) => 3,
                limine::MemmapValidationError::UnsortedOrOverlapping(_) => 4,
            };

            serial_write_u64(code);
            serial_write("\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Read-only paging mode discovery
    serial_write("[MM] Checking active paging mode...\r\n");

    let cr4: u64;

    unsafe {
        asm!(
            "mov {}, cr4",
            out(reg) cr4,
            options(nomem, nostack, preserves_flags)
        );
    }

    serial_write("[MM] CR4 value: ");
    serial_write_u64(cr4);
    serial_write("\r\n");

    if (cr4 & (1 << 12)) == 0 {
        serial_write("[MM] Five-level paging (LA57): DISABLED\r\n");
        serial_write("[MM] Active paging mode: FOUR-LEVEL\r\n");
    } else {
        serial_write("[MM] Five-level paging (LA57): ENABLED\r\n");
        serial_write("[MM] Active paging mode: FIVE-LEVEL\r\n");
    }


    // CPU physical address width discovery
    serial_write("[MM] Checking CPU physical address width...\r\n");

    match physical_address_width() {
        Some(width) => {
            serial_write("[MM] Physical address width: ");
            serial_write_u64(width as u64);
            serial_write(" bits\r\n");
        }

        None => {
            serial_write(
                "[MM] Physical address width unavailable or invalid.\r\n"
            );
        }
    }

    
    
    // Read-only active page-table root discovery
   
    // Read-only active page-table root discovery and validation
    serial_write("[MM] Validating active CR3 register...\r\n");

    // Reject five-level paging because this implementation expects PML4.
    if (cr4 & (1 << 12)) != 0 {
        serial_write("[MM] ERROR: Five-level paging is unsupported.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // Detect whether PCID is enabled.
    let pcide_enabled = (cr4 & (1 << 17)) != 0;

    serial_write("[MM] PCIDE: ");
    serial_write_u64(pcide_enabled as u64);
    serial_write("\r\n");

    // Obtain the CPU physical-address width.
    let address_width = match physical_address_width() {
        Some(width) => width,
        None => {
            serial_write("[MM] ERROR: Physical address width unavailable.\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    let address_mask = match physical_address_mask(address_width) {
        Some(mask) => mask,
        None => {
            serial_write("[MM] ERROR: Physical address mask unavailable.\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // Read CR3.
    let cr3: u64;

    unsafe {
        asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack, preserves_flags)
        );
    }

    serial_write("[MM] CR3 raw value: ");
    serial_write_u64(cr3);
    serial_write("\r\n");

    // When PCIDE is disabled, only PWT (bit 3) and PCD (bit 4)
    // are permitted in the low 12 bits.
    if !pcide_enabled && (cr3 & 0xFE7) != 0 {
        serial_write("[MM] ERROR: Reserved CR3 low bits are set.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // Extract the physical-address field.
    let address_field = cr3 & 0x000F_FFFF_FFFF_F000;

    // Reject physical-address bits above MAXPHYADDR and reserved
    // upper bits outside the physical-address field.
    if (address_field & !address_mask) != 0
        || (cr3 & !(address_mask | 0xFFF)) != 0
    {
        serial_write("[MM] ERROR: CR3 contains unsupported address bits.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    let root_page = address_field & address_mask;

    if root_page == 0 || root_page % limine::PAGE_SIZE != 0 {
        serial_write("[MM] ERROR: Invalid CR3 root address.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    serial_write("[MM] CR3 physical root: ");
    serial_write_u64(root_page);
    serial_write("\r\n");

    // Verify that the complete root page belongs to one memory-map entry.
    let root_end = match root_page.checked_add(limine::PAGE_SIZE) {
        Some(end) => end,
        None => {
            serial_write("[MM] ERROR: CR3 root range overflow.\r\n");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    let mut root_classified = false;
    let mut map_index = 0;

    if let Some((_, entry_count, _)) = limine::memmap_response_info() {
        while map_index < entry_count {
            if let Some((base, length, entry_type)) =
                limine::memmap_entry_at(map_index)
            {
                if let Some(entry_end) = base.checked_add(length) {
                    if root_page >= base && root_end <= entry_end {
                        serial_write("[MM] Root memory type: ");
                        serial_write_u64(entry_type);
                        serial_write("\r\n");

                        root_classified = true;
                        break;
                    }
                }
            }

            map_index += 1;
        }
    }

    if !root_classified {
        serial_write("[MM] ERROR: CR3 root is not classified.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    serial_write("[MM] CR3 validation PASSED.\r\n");


    // Read-only PML4 inspection
    serial_write("[MM] Inspecting PML4 root entries...\r\n");

   
    match limine::physical_to_virtual(root_page) {
        Some(_) => {
           let pml4_virtual = match limine::physical_range_to_virtual(
    root_page,
    limine::PAGE_SIZE,
) {
    Some(address) => address,
    None => {
        serial_write("[MM] ERROR: PML4 HHDM range validation failed.\r\n");
        loop {
            core::hint::spin_loop();
        }
    }
};

let pml4 = pml4_virtual as *const u64;
serial_write("[MM] PML4 HHDM range validation PASSED.\r\n");

            
            let mut present_count = 0u64;
            let mut index = 0usize;

            while index < 512 {
                let entry = unsafe {
                    core::ptr::read_volatile(pml4.add(index))
                };

                if (entry & 1) != 0 {
                    present_count += 1;
                }

                index += 1;
            }

            serial_write("[MM] PML4 entries inspected: 512\r\n");
            serial_write("[MM] Present PML4 entries: ");
            serial_write_u64(present_count);
            serial_write("\r\n");
            serial_write("[MM] PML4 root inspection completed.\r\n");

            // Inspect present entries without dereferencing child tables.
           
            // Validate present PML4 entries without dereferencing children.
            serial_write("[MM] Validating present PML4 entries...\r\n");

            let address_mask = match physical_address_width()
                .and_then(physical_address_mask)
            {
                Some(mask) => mask,
                None => {
                    serial_write("[MM] ERROR: Physical address mask unavailable.\r\n");
                    loop {
                        core::hint::spin_loop();
                    }
                }
            };

            let nx_active = nx_enabled();
            let mut validation_failed = false;

            index = 0;

            while index < 512 {
                let entry = unsafe {
                    core::ptr::read_volatile(pml4.add(index))
                };

                if (entry & 1) != 0 {
                    serial_write("[MM] PML4 index: ");
                    serial_write_u64(index as u64);
                    serial_write(" | Raw entry: ");
                    serial_write_u64(entry);
                    serial_write("\r\n");

                    // PS is reserved in a PML4 entry.
                    if (entry & (1 << 7)) != 0 {
                        serial_write("[MM] ERROR: Reserved PS bit is set.\r\n");
                        validation_failed = true;
                    }

                    // NX is reserved when EFER.NXE is disabled.
                    if (entry & (1u64 << 63)) != 0 && !nx_active {
                        serial_write("[MM] ERROR: NX set while NXE is disabled.\r\n");
                        validation_failed = true;
                    }

                    let address_bits =
                        entry & 0x000F_FFFF_FFFF_F000;

                    if (address_bits & !address_mask) != 0 {
                        serial_write("[MM] ERROR: Physical address exceeds CPU width.\r\n");
                        validation_failed = true;
                    } else if (entry & (1 << 7)) == 0 {
                        let child_physical =
                            address_bits & address_mask;

                        let child_end = match child_physical
                            .checked_add(limine::PAGE_SIZE)
                        {
                            Some(end) => end,
                            None => {
                                serial_write("[MM] ERROR: Child range overflow.\r\n");
                                validation_failed = true;
                                index += 1;
                                continue;
                            }
                        };

                        let mut classified = false;
                        let mut map_index = 0;

                        if let Some((_, count, _)) =
                            limine::memmap_response_info()
                        {
                            while map_index < count {
                                if let Some((base, length, kind)) =
                                    limine::memmap_entry_at(map_index)
                                {
                                    if let Some(end) = base.checked_add(length) {
                                        if child_physical >= base
                                            && child_end <= end
                                        {
                                            serial_write("[MM] Child table physical address: ");
                                            serial_write_u64(child_physical);
                                            serial_write("\r\n[MM] Child table memory type: ");
                                            serial_write_u64(kind);
                                            serial_write("\r\n");

                                            classified = true;
                                            break;
                                        }
                                    }
                                }

                                map_index += 1;
                            }
                        }

                        if !classified {
                            serial_write("[MM] ERROR: Child table is not classified.\r\n");
                            validation_failed = true;
                        }
                    }

                    serial_write("[MM] Writable: ");
                    serial_write_u64((entry >> 1) & 1);
                    serial_write(" | User: ");
                    serial_write_u64((entry >> 2) & 1);
                    serial_write(" | NX: ");
                    serial_write_u64((entry >> 63) & 1);
                    serial_write("\r\n");
                }

                index += 1;
            }

            if validation_failed {
                serial_write("[MM] PML4 validation FAILED.\r\n");
                loop {
                    core::hint::spin_loop();
                }
            }

            serial_write("[MM] PML4 validation PASSED.\r\n");

         serial_write("[MM] Inspecting child PDPT tables...\r\n");

let mut pdpt_count = 0u64;
let mut pml4_index = 0usize;
            
 let mut leaf_entries = 0u64;
let mut writable_entries = 0u64;
let mut user_entries = 0u64;
let mut nx_entries = 0u64;

let mut validated_1g = 0u64;
let mut validated_2m = 0u64;
let mut validated_4k = 0u64;
            
while pml4_index < 512 {
    let pml4_entry =
        unsafe { core::ptr::read_volatile(pml4.add(pml4_index)) };

    if (pml4_entry & 1) != 0 {

let pml4_writable = (pml4_entry & (1 << 1)) != 0;
let pml4_user = (pml4_entry & (1 << 2)) != 0;
let pml4_nx = (pml4_entry & (1u64 << 63)) != 0;
        
        let child_physical =
            pml4_entry & 0x000F_FFFF_FFFF_F000;

        let child_virtual =
            match limine::physical_range_to_virtual(
                child_physical,
                limine::PAGE_SIZE,
            ) {
                Some(address) => address,
                None => {
                    serial_write(
                        "[MM] ERROR: PDPT HHDM validation failed.\r\n"
                    );
                    loop {
                        core::hint::spin_loop();
                    }
                }
            };

        let pdpt = child_virtual as *const u64;
        let mut present_entries = 0u64;
        let mut entry_index = 0usize;

       while entry_index < 512 {
    let entry =
        unsafe {
            core::ptr::read_volatile(pdpt.add(entry_index))
        };

    if (entry & 1) != 0 {
        present_entries += 1;

        let effective_writable =
    pml4_writable && (entry & (1 << 1)) != 0;

let effective_user =
    pml4_user && (entry & (1 << 2)) != 0;

let effective_nx =
    pml4_nx || (entry & (1u64 << 63)) != 0;

        let writable = (entry >> 1) & 1;
        let user = (entry >> 2) & 1;
        let page_size = (entry >> 7) & 1;
        let nx = (entry >> 63) & 1;

        let address_bits =
            entry & 0x000F_FFFF_FFFF_F000;

        let address_mask = match physical_address_width()
            .and_then(physical_address_mask)
        {
            Some(mask) => mask,
            None => {
                serial_write(
                    "[MM] ERROR: Physical address mask unavailable.\r\n"
                );
                loop {
                    core::hint::spin_loop();
                }
            }
        };

        if (address_bits & !address_mask) != 0 {
            serial_write(
                "[MM] ERROR: PDPT entry has out-of-range address bits.\r\n"
            );
            loop {
                core::hint::spin_loop();
            }
        }

        if nx != 0 && !nx_enabled() {
            serial_write(
                "[MM] ERROR: NX set but NX is unavailable.\r\n"
            );
            loop {
                core::hint::spin_loop();
            }
        }

        serial_write("[MM] PDPT entry index: ");
        serial_write_u64(entry_index as u64);
        serial_write("\r\n");

        serial_write("[MM] Writable: ");
        serial_write_u64(writable);
        serial_write(" | User: ");
        serial_write_u64(user);
        serial_write(" | NX: ");
        serial_write_u64(nx);
        serial_write("\r\n");

        if page_size != 0 {

           if !supports_1g_pages() {
    serial_write(
        "[MM] ERROR: CPU does not support 1 GiB pages.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}

// Bit 12 is the PAT selector for a 1 GiB page.
if (entry & (1u64 << 12)) != 0 && !supports_pat() {
    serial_write(
        "[MM] ERROR: 1 GiB page uses PAT but CPU lacks PAT support.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}
            // 1 GiB huge-page mapping.
            let reserved_address_bits =
                entry & 0x0000_0000_3FFF_E000;

            if reserved_address_bits != 0 {
                serial_write(
                    "[MM] ERROR: Invalid 1 GiB page alignment.\r\n"
                );
                loop {
                    core::hint::spin_loop();
                }
            }

leaf_entries += 1;

if effective_writable {
    writable_entries += 1;
}

if effective_user {
    user_entries += 1;
}

if effective_nx {
    nx_entries += 1;
}

            
            let physical_base =
                address_bits & !((1u64 << 30) - 1);

            if !validate_leaf_physical_range(
    physical_base,
    1u64 << 30,
    address_width,
) {
    serial_write(
        "[MM] ERROR: 1 GiB physical mapping validation failed.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}

validated_1g += 1;

            

            serial_write("[MM] Mapping type: 1 GiB HUGE PAGE\r\n");
            serial_write("[MM] Physical base: ");
            serial_write_u64(physical_base);
            serial_write("\r\n");
        } else {
            // Pointer to a page directory.
            let directory_physical =
                address_bits & address_mask;

            if directory_physical == 0 {
                serial_write(
                    "[MM] ERROR: Invalid page-directory address.\r\n"
                );
                loop {
                    core::hint::spin_loop();
                }
            }

             match limine::physical_range_to_virtual(
                directory_physical,
                limine::PAGE_SIZE,
            ) {
                Some(directory_virtual) => {
                    serial_write(
                        "[MM] Mapping type: PAGE DIRECTORY POINTER\r\n"
                    );
                    serial_write(
                        "[MM] Page-directory HHDM range validation PASSED.\r\n"
                    );

                    let directory = directory_virtual as *const u64;

                    let mut directory_present = 0u64;
                    let mut huge_pages = 0u64;
                    let mut page_table_pointers = 0u64;

                    let address_mask = match physical_address_width()
                        .and_then(physical_address_mask)
                    {
                        Some(mask) => mask,
                        None => {
                            serial_write(
                                "[MM] ERROR: Address mask unavailable.\r\n"
                            );
                            loop {
                                core::hint::spin_loop();
                            }
                        }
                    };

                    let mut directory_index = 0usize;

                    while directory_index < 512 {
                        let entry = unsafe {
                            core::ptr::read_volatile(
                                directory.add(directory_index)
                            )
                        };

                        if (entry & 1) != 0 {
                            directory_present += 1;

                            let pde_writable =
    effective_writable && (entry & (1 << 1)) != 0;

let pde_user =
    effective_user && (entry & (1 << 2)) != 0;

let pde_nx =
    effective_nx || (entry & (1u64 << 63)) != 0;

                            let writable = (entry >> 1) & 1;
                            let user = (entry >> 2) & 1;
                            let page_size = (entry >> 7) & 1;
                            let nx = (entry >> 63) & 1;

                            let address_bits =
                                entry & 0x000F_FFFF_FFFF_F000;

                            if (address_bits & !address_mask) != 0 {
                                serial_write(
                                    "[MM] ERROR: PDE address exceeds physical width.\r\n"
                                );
                                loop {
                                    core::hint::spin_loop();
                                }
                            }

                            if nx != 0 && !nx_enabled() {
                                serial_write(
                                    "[MM] ERROR: PDE NX bit unsupported.\r\n"
                                );
                                loop {
                                    core::hint::spin_loop();
                                }
                            }

                            if page_size != 0 {

                                if !supports_2m_pages() {
    serial_write(
        "[MM] ERROR: CPU does not support 2 MiB pages.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}

// Bit 12 is the PAT selector for a 2 MiB page.
if (entry & (1u64 << 12)) != 0 && !supports_pat() {
    serial_write(
        "[MM] ERROR: 2 MiB page uses PAT but CPU lacks PAT support.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}
                                // 2 MiB huge-page mapping.
                                let reserved_bits =
                                    entry & 0x0000_0000_001F_E000;

                                if reserved_bits != 0 {
                                    serial_write(
                                        "[MM] ERROR: Invalid 2 MiB page alignment.\r\n"
                                    );
                                    loop {
                                        core::hint::spin_loop();
                                    }
                                }

let physical_base =
    address_bits & !((1u64 << 21) - 1);

if !validate_leaf_physical_range(
    physical_base,
    1u64 << 21,
    address_width,
) {
    serial_write(
        "[MM] ERROR: 2 MiB physical mapping validation failed.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}

validated_2m += 1;

                                
                               huge_pages += 1;

leaf_entries += 1;

if pde_writable {
    writable_entries += 1;
}

if pde_user {
    user_entries += 1;
}

if pde_nx {
    nx_entries += 1;
}
                                
                            } else {
                                // Pointer to a page table.
                                let table_physical =
                                    address_bits & address_mask;

                                if table_physical
                                    & (limine::PAGE_SIZE - 1) != 0
                                {
                                    serial_write(
                                        "[MM] ERROR: Page-table address is unaligned.\r\n"
                                    );
                                    loop {
                                        core::hint::spin_loop();
                                    }
                                }

                                match limine::physical_range_to_virtual(
    table_physical,
    limine::PAGE_SIZE,
) {
    Some(table_virtual) => {
        page_table_pointers += 1;

        serial_write(
            "[MM] Page-table HHDM validation PASSED.\r\n"
        );

        let page_table = table_virtual as *const u64;

        let mut table_present = 0u64;
        let mut table_index = 0usize;

        while table_index < 512 {
            let entry = unsafe {
                core::ptr::read_volatile(
                    page_table.add(table_index)
                )
            };

            if (entry & 1) != 0 {
                table_present += 1;

let pte_writable =
    pde_writable && (entry & (1 << 1)) != 0;

let pte_user =
    pde_user && (entry & (1 << 2)) != 0;

let pte_nx =
    pde_nx || (entry & (1u64 << 63)) != 0;

leaf_entries += 1;

if pte_writable {
    writable_entries += 1;
}

if pte_user {
    user_entries += 1;
}

if pte_nx {
    nx_entries += 1;
}
                

                let nx = (entry >> 63) & 1;

// Bit 7 is the PAT selector in a 4 KiB PTE.
if (entry & (1u64 << 7)) != 0 && !supports_pat() {
    serial_write(
        "[MM] ERROR: 4 KiB page uses PAT but CPU lacks PAT support.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}
                
                let address_bits =
                    entry & 0x000F_FFFF_FFFF_F000;

                // Validate physical address width.
                if (address_bits & !address_mask) != 0 {
                    serial_write(
                        "[MM] ERROR: PTE address exceeds physical width.\r\n"
                    );
                    loop {
                        core::hint::spin_loop();
                    }
                }

                // Validate NX support.
                if nx != 0 && !nx_enabled() {
                    serial_write(
                        "[MM] ERROR: PTE NX bit unsupported.\r\n"
                    );
                    loop {
                        core::hint::spin_loop();
                    }
                }


let physical_base = address_bits & address_mask;

if !validate_leaf_physical_range(
    physical_base,
    limine::PAGE_SIZE,
    address_width,
) {
    serial_write(
        "[MM] ERROR: 4 KiB physical mapping validation failed.\r\n"
    );
    loop {
        core::hint::spin_loop();
    }
}

validated_4k += 1;

                
            }

            table_index += 1;
        }

        serial_write(
            "[MM] Page-table entries inspected: 512\r\n"
        );

        serial_write(
            "[MM] Present page-table entries: "
        );
        serial_write_u64(table_present);
        serial_write("\r\n");
    }

    None => {
        serial_write(
            "[MM] ERROR: Page-table HHDM validation failed.\r\n"
        );
        loop {
            core::hint::spin_loop();
        }
    }
}
                            }

                            let _ = (writable, user);
                        }

                        directory_index += 1;
                    }

                    serial_write(
                        "[MM] Page-directory entries inspected: 512\r\n"
                    );

                    serial_write(
                        "[MM] Present directory entries: "
                    );
                    serial_write_u64(directory_present);
                    serial_write("\r\n");

                    serial_write("[MM] 2 MiB huge pages: ");
                    serial_write_u64(huge_pages);
                    serial_write("\r\n");

                    serial_write(
                        "[MM] Page-table pointers validated: "
                    );
                    serial_write_u64(page_table_pointers);
                    serial_write("\r\n");
                }

                None => {
                    serial_write(
                        "[MM] ERROR: Page-directory HHDM validation failed.\r\n"
                    );
                    loop {
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }

    entry_index += 1;
}

        pdpt_count += 1;

        serial_write("[MM] PDPT for PML4 index: ");
        serial_write_u64(pml4_index as u64);
        serial_write("\r\n");

        serial_write("[MM] PDPT present entries: ");
        serial_write_u64(present_entries);
        serial_write("\r\n");
    }

    pml4_index += 1;
}

serial_write("[MM] PDPT inspection complete.\r\n");
serial_write("[MM] PDPT tables inspected: ");
serial_write_u64(pdpt_count);
serial_write("\r\n");  

serial_write("[MM] Effective permission analysis:\r\n");

serial_write("[MM] Leaf entries: ");
serial_write_u64(leaf_entries);
serial_write("\r\n");

serial_write("[MM] Writable leaf entries: ");
serial_write_u64(writable_entries);
serial_write("\r\n");

serial_write("[MM] User-accessible leaf entries: ");
serial_write_u64(user_entries);
serial_write("\r\n");

serial_write("[MM] NX leaf entries: ");
serial_write_u64(nx_entries);
serial_write("\r\n");

if writable_entries > leaf_entries
    || user_entries > leaf_entries
    || nx_entries > leaf_entries
{
    serial_write("[MM] ERROR: Permission statistics inconsistent.\r\n");
    loop {
        core::hint::spin_loop();
    }
}

serial_write("[MM] Physical mapping validation summary:\r\n");

serial_write("[MM] Validated 1 GiB mappings: ");
serial_write_u64(validated_1g);
serial_write("\r\n");

serial_write("[MM] Validated 2 MiB mappings: ");
serial_write_u64(validated_2m);
serial_write("\r\n");

serial_write("[MM] Validated 4 KiB mappings: ");
serial_write_u64(validated_4k);
serial_write("\r\n");

serial_write("[MM] Physical mapping validation PASSED.\r\n");

            
serial_write("[MM] Effective permission analysis PASSED.\r\n");
            

            serial_write(
                "[MM] Present PML4 entry inspection completed.\r\n"
            );
        }

        None => {
            serial_write("[MM] PML4 HHDM conversion failed.\r\n");
        }
    }


    
    
    // Physical memory statistics
    match limine::calculate_memmap_stats() {
        Ok(stats) => {
            serial_write("[MEM] Physical memory statistics:\r\n");

            serial_write("[MEM] Total usable bytes: ");
            serial_write_u64(stats.usable_bytes);
            serial_write("\r\n");

            serial_write("[MEM] Usable regions: ");
            serial_write_u64(stats.usable_region_count);
            serial_write("\r\n");

            serial_write("[MEM] Largest usable region: ");
            serial_write_u64(stats.largest_usable_region);
            serial_write("\r\n");

            serial_write("[MEM] Bootloader-reclaimable bytes: ");
            serial_write_u64(stats.bootloader_reclaimable_bytes);
            serial_write("\r\n");

            serial_write("[MEM] Bootloader-reclaimable regions: ");
            serial_write_u64(stats.bootloader_reclaimable_region_count);
            serial_write("\r\n");
        }

        Err(_) => {
            serial_write("[MEM] Statistics calculation FAILED.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Read Limine memory-map response
    match limine::memmap_response_info() {
        Some((revision, entry_count, _entries)) => {
            serial_write("[MEM] Response received.\r\n");

            serial_write("[MEM] Response revision: ");
            serial_write_u64(revision);

            serial_write("\r\n[MEM] Entry count: ");
            serial_write_u64(entry_count);
            serial_write("\r\n");

            // Read individual memory-map entries
            let mut index = 0;

            while index < entry_count {
                match limine::memmap_entry_at(index) {
                    Some((base, length, entry_type)) => {
                        serial_write("[MEM] Entry ");
                        serial_write_u64(index);

                        serial_write(" | Base: ");
                        serial_write_u64(base);

                        serial_write(" | Length: ");
                        serial_write_u64(length);

                        serial_write(" | Type: ");
                        serial_write_u64(entry_type);

                        serial_write("\r\n");
                    }

                    None => {
                        serial_write("[MEM] Invalid entry at index ");
                        serial_write_u64(index);
                        serial_write("\r\n");
                        break;
                    }
                }

                index += 1;
            }
        }

        None => {
            serial_write("[MEM] Invalid or missing response.\r\n");
        }
    }

    // Kernel banner
    serial_write("\r\n");
    serial_write("================================\r\n");
    serial_write("      ASTERNEL KERNEL v0.1\r\n");
    serial_write("      x86_64 architecture\r\n");
    serial_write("      Serial console: COM1\r\n");
    serial_write("================================\r\n");
    serial_write("Kernel entry successful.\r\n");

    loop {
        core::hint::spin_loop();
    }
}

// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial_write("\r\nKERNEL PANIC\r\n");

    loop {
        core::hint::spin_loop();
    }
}
