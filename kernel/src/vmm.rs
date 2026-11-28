pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const VIRTUAL_ADDRESS_BITS: u8 = 48;

const PAGE_OFFSET_MASK: u64 = PAGE_SIZE - 1;
const INDEX_MASK: u64 = 0x1FF;

const FLAG_PRESENT: u64 = 1 << 0;
const FLAG_WRITABLE: u64 = 1 << 1;
const FLAG_USER: u64 = 1 << 2;
const FLAG_WRITE_THROUGH: u64 = 1 << 3;
const FLAG_CACHE_DISABLE: u64 = 1 << 4;
const FLAG_ACCESSED: u64 = 1 << 5;
const FLAG_DIRTY: u64 = 1 << 6;
const FLAG_BIT_7: u64 = 1 << 7;
const FLAG_GLOBAL: u64 = 1 << 8;
const FLAG_NO_EXECUTE: u64 = 1 << 63;

const PHYSICAL_ADDRESS_FIELD_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmmError {
    NonCanonicalVirtualAddress,
    InvalidPhysicalAddressWidth,
    PhysicalAddressOutOfRange,
    InvalidPageTableIndex,
    InvalidPageSizeEncoding,
    UnmappedVirtualAddress,
    PageTableNotAccessible,
    InvalidPageTableAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct VirtualAddress(u64);

impl VirtualAddress {
    pub fn new(raw: u64) -> Result<Self, VmmError> {
        if !is_canonical(raw) {
            return Err(VmmError::NonCanonicalVirtualAddress);
        }

        Ok(Self(raw))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn page_offset(self) -> u16 {
        (self.0 & PAGE_OFFSET_MASK) as u16
    }

    pub const fn page_aligned(self) -> u64 {
        self.0 & !PAGE_OFFSET_MASK
    }

    pub const fn indices(self) -> PageTableIndices {
        PageTableIndices {
            pml4: ((self.0 >> 39) & INDEX_MASK) as u16,
            pdpt: ((self.0 >> 30) & INDEX_MASK) as u16,
            pd: ((self.0 >> 21) & INDEX_MASK) as u16,
            pt: ((self.0 >> 12) & INDEX_MASK) as u16,
            offset: (self.0 & PAGE_OFFSET_MASK) as u16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub fn new(raw: u64, address_width: u8) -> Result<Self, VmmError> {
        if !(36..=52).contains(&address_width) {
            return Err(VmmError::InvalidPhysicalAddressWidth);
        }

        let limit = 1u64
            .checked_shl(address_width as u32)
            .ok_or(VmmError::InvalidPhysicalAddressWidth)?;

        if raw >= limit {
            return Err(VmmError::PhysicalAddressOutOfRange);
        }

        Ok(Self(raw))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn page_aligned(self) -> bool {
        (self.0 & PAGE_OFFSET_MASK) == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTableIndices {
    pub pml4: u16,
    pub pdpt: u16,
    pub pd: u16,
    pub pt: u16,
    pub offset: u16,
}

impl PageTableIndices {
    pub fn validate(self) -> Result<(), VmmError> {
        if self.pml4 >= 512
            || self.pdpt >= 512
            || self.pd >= 512
            || self.pt >= 512
            || self.offset >= PAGE_SIZE as u16
        {
            return Err(VmmError::InvalidPageTableIndex);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    Size4KiB,
    Size2MiB,
    Size1GiB,
}

impl PageSize {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Size4KiB => 4 * 1024,
            Self::Size2MiB => 2 * 1024 * 1024,
            Self::Size1GiB => 1024 * 1024 * 1024,
        }
    }

    pub const fn alignment_mask(self) -> u64 {
        self.bytes() - 1
    }

    pub const fn contains_aligned(self, address: u64) -> bool {
        (address & self.alignment_mask()) == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageFlags(u64);

impl PageFlags {
    pub const fn from_entry(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn present(self) -> bool {
        (self.0 & FLAG_PRESENT) != 0
    }

    pub const fn writable(self) -> bool {
        (self.0 & FLAG_WRITABLE) != 0
    }

    pub const fn user_accessible(self) -> bool {
        (self.0 & FLAG_USER) != 0
    }

    pub const fn write_through(self) -> bool {
        (self.0 & FLAG_WRITE_THROUGH) != 0
    }

    pub const fn cache_disable(self) -> bool {
        (self.0 & FLAG_CACHE_DISABLE) != 0
    }

    pub const fn accessed(self) -> bool {
        (self.0 & FLAG_ACCESSED) != 0
    }

    pub const fn dirty(self) -> bool {
        (self.0 & FLAG_DIRTY) != 0
    }

    // Bit 7 means PS in PDPTE/PDE entries, but PAT in a 4 KiB PTE.
    // Keep its interpretation dependent on the entry level.
    pub const fn bit_7_set(self) -> bool {
        (self.0 & FLAG_BIT_7) != 0
    }

    pub const fn global(self) -> bool {
        (self.0 & FLAG_GLOBAL) != 0
    }

    pub const fn no_execute(self) -> bool {
        (self.0 & FLAG_NO_EXECUTE) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const EMPTY: Self = Self(0);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn flags(self) -> PageFlags {
        PageFlags::from_entry(self.0)
    }

    pub const fn is_present(self) -> bool {
        self.flags().present()
    }

    // Decode a present entry's physical base for the specified leaf size.
    // For 2 MiB and 1 GiB leaves, bit 12 is PAT and must not be
    // interpreted as part of the physical base address.
    pub fn physical_base(
        self,
        page_size: PageSize,
        address_width: u8,
    ) -> Result<Option<PhysicalAddress>, VmmError> {
        if !self.is_present() {
            return Ok(None);
        }

        if !(36..=52).contains(&address_width) {
            return Err(VmmError::InvalidPhysicalAddressWidth);
        }

        if page_size != PageSize::Size4KiB
            && (self.0 & FLAG_BIT_7) == 0
        {
            return Err(VmmError::InvalidPageSizeEncoding);
        }

        let address_mask = ((1u64 << address_width) - 1)
            & PHYSICAL_ADDRESS_FIELD_MASK;
        let address_field = self.0 & PHYSICAL_ADDRESS_FIELD_MASK;
        let pat_mask = match page_size {
            PageSize::Size4KiB => 0,
            PageSize::Size2MiB | PageSize::Size1GiB => 1 << 12,
        };
        let reserved_alignment_mask =
            page_size.alignment_mask() & !PAGE_OFFSET_MASK & !pat_mask;

        if (address_field & !address_mask & !pat_mask) != 0
            || (address_field & reserved_alignment_mask) != 0
        {
            return Err(VmmError::PhysicalAddressOutOfRange);
        }

        let base = address_field
            & address_mask
            & !page_size.alignment_mask();

        Ok(Some(PhysicalAddress(base)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslationPermissions {
    pub writable: bool,
    pub user_accessible: bool,
    pub executable: bool,
    pub global: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualTranslation {
    pub physical_address: PhysicalAddress,
    pub page_size: PageSize,
    pub permissions: TranslationPermissions,
}

fn finish_translation(
    entry: PageTableEntry,
    page_size: PageSize,
    virtual_address: VirtualAddress,
    address_width: u8,
    writable: bool,
    user_accessible: bool,
    no_execute: bool,
) -> Result<VirtualTranslation, VmmError> {
    let base = entry
        .physical_base(page_size, address_width)?
        .ok_or(VmmError::UnmappedVirtualAddress)?
        .raw();
    let offset = virtual_address.raw() & page_size.alignment_mask();
    let physical = base
        .checked_add(offset)
        .ok_or(VmmError::PhysicalAddressOutOfRange)?;
    let physical_address = PhysicalAddress::new(physical, address_width)?;

    Ok(VirtualTranslation {
        physical_address,
        page_size,
        permissions: TranslationPermissions {
            writable,
            user_accessible,
            executable: !no_execute,
            global: entry.flags().global(),
        },
    })
}

/// Read-only walk of a four-level x86-64 page-table hierarchy.
/// The callback returns None when a complete table page cannot be read.
pub fn translate_virtual_address<F>(
    root_physical: u64,
    virtual_address: u64,
    address_width: u8,
    mut read_entry: F,
) -> Result<VirtualTranslation, VmmError>
where
    F: FnMut(u64, usize) -> Option<u64>,
{
    let root = PhysicalAddress::new(root_physical, address_width)?;
    if !root.page_aligned() || root.raw() == 0 {
        return Err(VmmError::InvalidPageTableAddress);
    }

    let virtual_address = VirtualAddress::new(virtual_address)?;
    let indices = virtual_address.indices();
    let mut table_physical = root.raw();
    let mut writable = true;
    let mut user_accessible = true;
    let mut no_execute = false;

    let pml4e = PageTableEntry::from_raw(
        read_entry(table_physical, indices.pml4 as usize)
            .ok_or(VmmError::PageTableNotAccessible)?,
    );
    if !pml4e.is_present() {
        return Err(VmmError::UnmappedVirtualAddress);
    }
    if pml4e.flags().bit_7_set() {
        return Err(VmmError::InvalidPageSizeEncoding);
    }
    writable &= pml4e.flags().writable();
    user_accessible &= pml4e.flags().user_accessible();
    no_execute |= pml4e.flags().no_execute();
    table_physical = pml4e
        .physical_base(PageSize::Size4KiB, address_width)?
        .ok_or(VmmError::UnmappedVirtualAddress)?
        .raw();

    let pdpte = PageTableEntry::from_raw(
        read_entry(table_physical, indices.pdpt as usize)
            .ok_or(VmmError::PageTableNotAccessible)?,
    );
    if !pdpte.is_present() {
        return Err(VmmError::UnmappedVirtualAddress);
    }
    writable &= pdpte.flags().writable();
    user_accessible &= pdpte.flags().user_accessible();
    no_execute |= pdpte.flags().no_execute();

    if pdpte.flags().bit_7_set() {
        return finish_translation(
            pdpte, PageSize::Size1GiB, virtual_address, address_width,
            writable, user_accessible, no_execute,
        );
    }
    table_physical = pdpte
        .physical_base(PageSize::Size4KiB, address_width)?
        .ok_or(VmmError::UnmappedVirtualAddress)?
        .raw();

    let pde = PageTableEntry::from_raw(
        read_entry(table_physical, indices.pd as usize)
            .ok_or(VmmError::PageTableNotAccessible)?,
    );
    if !pde.is_present() {
        return Err(VmmError::UnmappedVirtualAddress);
    }
    writable &= pde.flags().writable();
    user_accessible &= pde.flags().user_accessible();
    no_execute |= pde.flags().no_execute();

    if pde.flags().bit_7_set() {
        return finish_translation(
            pde, PageSize::Size2MiB, virtual_address, address_width,
            writable, user_accessible, no_execute,
        );
    }
    table_physical = pde
        .physical_base(PageSize::Size4KiB, address_width)?
        .ok_or(VmmError::UnmappedVirtualAddress)?
        .raw();

    // For a 4 KiB PTE, bit 7 is PAT rather than PS.
    let pte = PageTableEntry::from_raw(
        read_entry(table_physical, indices.pt as usize)
            .ok_or(VmmError::PageTableNotAccessible)?,
    );
    if !pte.is_present() {
        return Err(VmmError::UnmappedVirtualAddress);
    }
    writable &= pte.flags().writable();
    user_accessible &= pte.flags().user_accessible();
    no_execute |= pte.flags().no_execute();

    finish_translation(
        pte, PageSize::Size4KiB, virtual_address, address_width,
        writable, user_accessible, no_execute,
    )
}

/// Synthetic checks for all supported leaf sizes, permission propagation,
/// and a non-present mapping. These tests never touch active page tables.
pub fn validate_translation_walk() -> bool {
    let virtual_raw = 0x0000_1234_5678_9678u64;
    let virtual_address = match VirtualAddress::new(virtual_raw) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let ix = virtual_address.indices();
    let flags = FLAG_PRESENT | FLAG_WRITABLE | FLAG_USER;

    let result_4k = translate_virtual_address(0x1000, virtual_raw, 40, |table, index| {
        if table == 0x1000 && index == ix.pml4 as usize {
            Some(0x2000 | flags)
        } else if table == 0x2000 && index == ix.pdpt as usize {
            Some(0x3000 | flags)
        } else if table == 0x3000 && index == ix.pd as usize {
            Some(0x4000 | flags)
        } else if table == 0x4000 && index == ix.pt as usize {
            Some(0x0056_7000 | flags | FLAG_GLOBAL)
        } else {
            None
        }
    });
    match result_4k {
        Ok(r) if r.physical_address.raw() == 0x0056_7678
            && r.page_size == PageSize::Size4KiB
            && r.permissions.writable && r.permissions.user_accessible
            && r.permissions.executable && r.permissions.global => {}
        _ => return false,
    }

    let result_2m = translate_virtual_address(0x1000, virtual_raw, 40, |table, index| {
        if table == 0x1000 && index == ix.pml4 as usize {
            Some(0x2000 | flags)
        } else if table == 0x2000 && index == ix.pdpt as usize {
            Some(0x3000 | flags)
        } else if table == 0x3000 && index == ix.pd as usize {
            Some(0x0200_0000 | flags | FLAG_BIT_7)
        } else {
            None
        }
    });
    match result_2m {
        Ok(r) if r.physical_address.raw()
            == 0x0200_0000 + (virtual_raw & 0x1F_FFFF)
            && r.page_size == PageSize::Size2MiB => {}
        _ => return false,
    }

    let result_1g = translate_virtual_address(0x1000, virtual_raw, 40, |table, index| {
        if table == 0x1000 && index == ix.pml4 as usize {
            Some(0x2000 | flags)
        } else if table == 0x2000 && index == ix.pdpt as usize {
            Some(0x4000_0000 | flags | FLAG_BIT_7)
        } else {
            None
        }
    });
    match result_1g {
        Ok(r) if r.physical_address.raw()
            == 0x4000_0000 + (virtual_raw & 0x3FFF_FFFF)
            && r.page_size == PageSize::Size1GiB => {}
        _ => return false,
    }

    let restricted = translate_virtual_address(0x1000, virtual_raw, 40, |table, index| {
        if table == 0x1000 && index == ix.pml4 as usize {
            Some(0x2000 | FLAG_PRESENT | FLAG_WRITABLE)
        } else if table == 0x2000 && index == ix.pdpt as usize {
            Some(0x3000 | FLAG_PRESENT | FLAG_NO_EXECUTE)
        } else if table == 0x3000 && index == ix.pd as usize {
            Some(0x4000 | flags)
        } else if table == 0x4000 && index == ix.pt as usize {
            Some(0x0056_7000 | flags)
        } else {
            None
        }
    });
    match restricted {
        Ok(r) if !r.permissions.writable && !r.permissions.user_accessible
            && !r.permissions.executable => {}
        _ => return false,
    }

    matches!(
        translate_virtual_address(0x1000, virtual_raw, 40, |_, _| Some(0)),
        Err(VmmError::UnmappedVirtualAddress)
    )
}

#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self {
            entries: [PageTableEntry::EMPTY; PAGE_TABLE_ENTRIES],
        }
    }

    pub fn entry(&self, index: usize) -> Option<PageTableEntry> {
        self.entries.get(index).copied()
    }

    pub fn present_entries(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_present())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.present_entries() == 0
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::empty()
    }
}

pub const fn is_canonical(address: u64) -> bool {
    let upper = address >> VIRTUAL_ADDRESS_BITS;
    let sign_bit = (address >> 47) & 1;

    if sign_bit == 0 {
        upper == 0
    } else {
        upper == 0xFFFF
    }
}

// Pure, deterministic tests. This stage defines and validates VMM
// primitives only; it does not modify Limine's active page tables.
pub fn validate_foundations() -> bool {
    let low_canonical = 0x0000_7FFF_FFFF_FFFF;
    let high_canonical = 0xFFFF_8000_0000_0000;
    let first_noncanonical_low = 0x0000_8000_0000_0000;
    let first_noncanonical_high = 0xFFFF_7FFF_FFFF_FFFF;

    if !is_canonical(low_canonical)
        || !is_canonical(high_canonical)
        || is_canonical(first_noncanonical_low)
        || is_canonical(first_noncanonical_high)
    {
        return false;
    }

    let low = match VirtualAddress::new(0x0000_0000_0000_1ABC) {
        Ok(address) => address,
        Err(_) => return false,
    };

    let low_indices = low.indices();
    if low_indices.pml4 != 0
        || low_indices.pdpt != 0
        || low_indices.pd != 0
        || low_indices.pt != 1
        || low_indices.offset != 0xABC
        || low_indices.validate().is_err()
        || low.page_offset() != 0xABC
        || low.page_aligned() != 0x1000
    {
        return false;
    }

    let high = match VirtualAddress::new(0xFFFF_FFFF_FFFF_FFFF) {
        Ok(address) => address,
        Err(_) => return false,
    };

    let high_indices = high.indices();
    if high_indices.pml4 != 511
        || high_indices.pdpt != 511
        || high_indices.pd != 511
        || high_indices.pt != 511
        || high_indices.offset != 0xFFF
        || high_indices.validate().is_err()
    {
        return false;
    }

    if VirtualAddress::new(first_noncanonical_low).is_ok()
        || VirtualAddress::new(first_noncanonical_high).is_ok()
    {
        return false;
    }

    let physical = match PhysicalAddress::new(0x1234_5000, 40) {
        Ok(address) => address,
        Err(_) => return false,
    };

    if physical.raw() != 0x1234_5000
        || !physical.page_aligned()
        || PhysicalAddress::new(1u64 << 40, 40).is_ok()
        || PhysicalAddress::new(0x1000, 35).is_ok()
    {
        return false;
    }

    if PageSize::Size4KiB.bytes() != 0x1000
        || PageSize::Size2MiB.bytes() != 0x20_0000
        || PageSize::Size1GiB.bytes() != 0x4000_0000
        || !PageSize::Size2MiB.contains_aligned(0x4000_0000)
        || PageSize::Size2MiB.contains_aligned(0x4000_1000)
    {
        return false;
    }

    let raw_flags = FLAG_PRESENT
        | FLAG_WRITABLE
        | FLAG_USER
        | FLAG_WRITE_THROUGH
        | FLAG_CACHE_DISABLE
        | FLAG_ACCESSED
        | FLAG_DIRTY
        | FLAG_BIT_7
        | FLAG_GLOBAL
        | FLAG_NO_EXECUTE;
    let flags = PageFlags::from_entry(raw_flags);

    if !flags.present()
        || !flags.writable()
        || !flags.user_accessible()
        || !flags.write_through()
        || !flags.cache_disable()
        || !flags.accessed()
        || !flags.dirty()
        || !flags.bit_7_set()
        || !flags.global()
        || !flags.no_execute()
    {
        return false;
    }

    let absent = PageTableEntry::from_raw(FLAG_WRITABLE | FLAG_NO_EXECUTE);
    if absent.is_present()
        || absent.physical_base(PageSize::Size4KiB, 40).ok() != Some(None)
    {
        return false;
    }

    let present = PageTableEntry::from_raw(
        0x1234_5000 | FLAG_PRESENT | FLAG_WRITABLE,
    );
    match present.physical_base(PageSize::Size4KiB, 40) {
        Ok(Some(address)) if address.raw() == 0x1234_5000 => {}
        _ => return false,
    }

    // Huge-page PAT (bit 12) must not leak into the physical base.
    let huge_2m = PageTableEntry::from_raw(
        0x2000_0000 | (1 << 12) | FLAG_PRESENT | FLAG_BIT_7,
    );
    match huge_2m.physical_base(PageSize::Size2MiB, 40) {
        Ok(Some(address)) if address.raw() == 0x2000_0000 => {}
        _ => return false,
    }

    let huge_1g = PageTableEntry::from_raw(
        0x4000_0000 | (1 << 12) | FLAG_PRESENT | FLAG_BIT_7,
    );
    match huge_1g.physical_base(PageSize::Size1GiB, 40) {
        Ok(Some(address)) if address.raw() == 0x4000_0000 => {}
        _ => return false,
    }

    let invalid_huge = PageTableEntry::from_raw(
        0x2000_0000 | FLAG_PRESENT,
    );
    if invalid_huge
        .physical_base(PageSize::Size2MiB, 40)
        .is_ok()
    {
        return false;
    }

    let table = PageTable::empty();
    if core::mem::size_of::<PageTable>() != PAGE_SIZE as usize
        || core::mem::align_of::<PageTable>() != PAGE_SIZE as usize
        || !table.is_empty()
        || table.present_entries() != 0
        || table.entry(0) != Some(PageTableEntry::EMPTY)
        || table.entry(PAGE_TABLE_ENTRIES).is_some()
    {
        return false;
    }

    true
}
