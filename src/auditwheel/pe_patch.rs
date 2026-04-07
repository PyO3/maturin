//! PE binary patching utilities for Windows wheel repair.
//!
//! Provides functions to modify PE (Portable Executable) files:
//! - Replace imported DLL names in the import directory table
//! - Clear DependentLoadFlags to ensure AddDllDirectory works
//! - Query PE architecture and linker version
//!
//! # PE Import Table Patching
//!
//! When DLLs are bundled with hash-suffixed names (e.g., `foo-ab12cd34.dll`),
//! the importing binary must be updated to reference the new name. Unlike
//! ELF (patchelf) and Mach-O (install_name_tool), there's no standard tool
//! for this on Windows. We implement it directly:
//!
//! 1. Parse the PE import directory table to find DLL name references
//! 2. Find space for new (longer) names in section padding
//! 3. Write new names and update import table RVAs
//! 4. Fix PE checksum and remove Authenticode signatures
//!
//! This matches [delvewheel](https://github.com/adang1345/delvewheel)'s
//! approach using the `pefile` Python library.

use anyhow::{Context, Result, bail};
use goblin::pe::header::{SIZEOF_COFF_HEADER, SIZEOF_PE_MAGIC};
use goblin::pe::import::SIZEOF_IMPORT_DIRECTORY_ENTRY;
use goblin::pe::section_table::{
    self, IMAGE_SCN_CNT_CODE, IMAGE_SCN_CNT_INITIALIZED_DATA, IMAGE_SCN_CNT_UNINITIALIZED_DATA,
    IMAGE_SCN_MEM_DISCARDABLE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ, SectionTable,
};
use std::path::Path;

use fs_err as fs;

// -- Byte reading/writing helpers --

fn read_u16_le(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("PE read out of bounds at offset {offset}"))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("PE read out of bounds at offset {offset}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn write_u16_le(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32_le(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_cstring(data: &[u8], offset: usize) -> Option<String> {
    let tail = data.get(offset..)?;
    let end = tail.iter().position(|&b| b == 0)?;
    String::from_utf8(data[offset..offset + end].to_vec()).ok()
}

// -- PE format constants --

const SECTION_HEADER_SIZE: usize = section_table::SIZEOF_SECTION_TABLE;

// Data directory indices
const DD_IMPORT: usize = 1;
const DD_SECURITY: usize = 4;
const DD_LOAD_CONFIG: usize = 10;
const DD_DELAY_IMPORT: usize = 13;

// Import descriptor name RVA field offset within IMAGE_IMPORT_DESCRIPTOR
const IMPORT_DESC_NAME_OFFSET: usize = 12;

// Delay import descriptor size (8 × u32)
const DELAY_IMPORT_DESC_SIZE: usize = 32;
// DllNameRVA field offset within IMAGE_DELAYLOAD_DESCRIPTOR
const DELAY_IMPORT_NAME_OFFSET: usize = 4;

// New section characteristics: readable initialized data
const NEW_SECTION_CHARS: u32 = IMAGE_SCN_MEM_READ | IMAGE_SCN_CNT_INITIALIZED_DATA;

// -- Parsed PE structures --

struct SectionEntry {
    table: SectionTable,
    /// File offset of this section's header entry
    header_offset: usize,
}

struct PeLayout {
    is_64bit: bool,
    /// Offset to the CheckSum field within the file
    checksum_offset: usize,
    /// Offset to the SizeOfInitializedData field
    size_of_init_data_offset: usize,
    /// Offset to the SizeOfImage field
    size_of_image_offset: usize,
    /// Offset to the SizeOfHeaders field
    size_of_headers_offset: usize,
    /// Offset to the NumberOfSections field in COFF header
    num_sections_offset: usize,
    /// Offset to the Data Directories array
    data_dirs_offset: usize,
    /// Number of data directories
    num_data_dirs: u32,
    /// Offset to the section table
    section_table_offset: usize,
    file_alignment: u32,
    section_alignment: u32,
    sections: Vec<SectionEntry>,
}

impl PeLayout {
    fn section_tables(&self) -> impl Iterator<Item = &SectionTable> {
        self.sections.iter().map(|s| &s.table)
    }
}

struct ImportRef {
    /// The DLL name as found in the PE
    dll_name: String,
    /// File offset of the Name/DllNameRVA field in the import descriptor
    name_field_offset: usize,
}

// -- PE parsing --

fn parse_pe_layout(data: &[u8]) -> Result<PeLayout> {
    let pe = goblin::pe::PE::parse(data).context("Failed to parse PE file")?;
    let header = &pe.header;
    let opt = header
        .optional_header
        .ok_or_else(|| anyhow::anyhow!("PE file has no optional header"))?;

    let is_64bit = pe.is_64;
    let pe_offset = header.dos_header.pe_pointer as usize;
    let coff_offset = pe_offset + SIZEOF_PE_MAGIC;
    let num_sections_offset = coff_offset + 2;
    let opt_hdr_offset = coff_offset + SIZEOF_COFF_HEADER;
    let opt_hdr_size = header.coff_header.size_of_optional_header as usize;

    // Compute raw byte offsets for fields we need to patch.
    // The standard fields start at opt_hdr_offset, followed by windows fields,
    // then data directories.
    // SizeOfInitializedData is at offset 8 in the optional header (both PE32 and PE32+)
    let size_of_init_data_offset = opt_hdr_offset + 8;
    // SizeOfImage is at windows_fields_offset + 24 (PE32) or windows_fields_offset + 28 (PE32+)
    // In the raw optional header: offset 56 for both PE32 and PE32+
    let size_of_image_offset = opt_hdr_offset + 56;
    // SizeOfHeaders: offset 60
    let size_of_headers_offset = opt_hdr_offset + 60;
    // CheckSum: offset 64
    let checksum_offset = opt_hdr_offset + 64;

    let data_dirs_offset = if is_64bit {
        opt_hdr_offset + 112
    } else {
        opt_hdr_offset + 96
    };
    let num_data_dirs = opt.windows_fields.number_of_rva_and_sizes;

    let section_table_offset = opt_hdr_offset + opt_hdr_size;

    let sections = pe
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| SectionEntry {
            table: s.clone(),
            header_offset: section_table_offset + i * SECTION_HEADER_SIZE,
        })
        .collect();

    Ok(PeLayout {
        is_64bit,
        checksum_offset,
        size_of_init_data_offset,
        size_of_image_offset,
        size_of_headers_offset,
        num_sections_offset,
        data_dirs_offset,
        num_data_dirs,
        section_table_offset,
        file_alignment: opt.windows_fields.file_alignment,
        section_alignment: opt.windows_fields.section_alignment,
        sections,
    })
}

fn rva_to_offset(rva: u32, sections: &[SectionTable]) -> Option<usize> {
    for s in sections {
        // Use max of virtual_size and size_of_raw_data for section ownership.
        // This handles BSS sections where virtual_size > size_of_raw_data.
        let span = s.virtual_size.max(s.size_of_raw_data);
        if rva >= s.virtual_address && rva < s.virtual_address.saturating_add(span) {
            let delta = rva - s.virtual_address;
            // Only return a file offset if within the raw data on disk.
            // RVAs in the zero-fill/BSS tail have no file backing.
            if delta < s.size_of_raw_data {
                return Some((s.pointer_to_raw_data + delta) as usize);
            } else {
                return None;
            }
        }
    }
    None
}

fn get_data_dir(data: &[u8], layout: &PeLayout, index: usize) -> Result<Option<(u32, u32)>> {
    if index as u32 >= layout.num_data_dirs {
        return Ok(None);
    }
    let off = layout.data_dirs_offset + index * 8;
    if off + 8 > data.len() {
        return Ok(None);
    }
    let rva = read_u32_le(data, off)?;
    let size = read_u32_le(data, off + 4)?;
    if rva == 0 && size == 0 {
        Ok(None)
    } else {
        Ok(Some((rva, size)))
    }
}

fn parse_import_descriptors(
    data: &[u8],
    sections: &[SectionTable],
    table_rva: u32,
    desc_size: usize,
    name_field_offset: usize,
    imports: &mut Vec<ImportRef>,
) -> Result<()> {
    let Some(table_offset) = rva_to_offset(table_rva, sections) else {
        return Ok(());
    };
    let mut off = table_offset;
    loop {
        if off + desc_size > data.len() {
            break;
        }
        let name_rva = read_u32_le(data, off + name_field_offset)?;
        if name_rva == 0 {
            break;
        }
        if let Some(name_offset) = rva_to_offset(name_rva, sections)
            && let Some(name) = read_cstring(data, name_offset)
        {
            imports.push(ImportRef {
                dll_name: name,
                name_field_offset: off + name_field_offset,
            });
        }
        off += desc_size;
    }
    Ok(())
}

fn parse_imports(data: &[u8], layout: &PeLayout) -> Result<Vec<ImportRef>> {
    let mut imports = Vec::new();
    let sections: Vec<_> = layout.section_tables().cloned().collect();

    // Regular imports (DataDir[1])
    if let Some((rva, _size)) = get_data_dir(data, layout, DD_IMPORT)? {
        parse_import_descriptors(
            data,
            &sections,
            rva,
            SIZEOF_IMPORT_DIRECTORY_ENTRY,
            IMPORT_DESC_NAME_OFFSET,
            &mut imports,
        )?;
    }

    // Delay-load imports (DataDir[13])
    if let Some((rva, _size)) = get_data_dir(data, layout, DD_DELAY_IMPORT)? {
        parse_import_descriptors(
            data,
            &sections,
            rva,
            DELAY_IMPORT_DESC_SIZE,
            DELAY_IMPORT_NAME_OFFSET,
            &mut imports,
        )?;
    }

    Ok(imports)
}

// -- Section padding utilities --

/// Check if a section is suitable for writing new DLL name strings.
fn is_section_writable(s: &SectionTable) -> bool {
    s.characteristics & IMAGE_SCN_CNT_CODE == 0
        && s.characteristics & IMAGE_SCN_CNT_INITIALIZED_DATA != 0
        && s.characteristics & IMAGE_SCN_CNT_UNINITIALIZED_DATA == 0
        && s.characteristics & IMAGE_SCN_MEM_DISCARDABLE == 0
        && s.characteristics & IMAGE_SCN_MEM_EXECUTE == 0
        && s.characteristics & IMAGE_SCN_MEM_READ != 0
}

/// Find padding slots in existing sections for new DLL name strings.
///
/// Uses the Next Fit bin packing algorithm (same as delvewheel).
/// Returns a mapping from new name index → (file_offset, rva) if all names fit.
fn find_padding_slots(sections: &[SectionEntry], new_names: &[&[u8]]) -> Option<Vec<(usize, u32)>> {
    let mut slots = Vec::with_capacity(new_names.len());
    let mut name_idx = 0;

    let mut sorted_sections: Vec<(usize, &SectionEntry)> = sections.iter().enumerate().collect();
    sorted_sections.sort_by_key(|(_, s)| s.table.virtual_address);

    for (si, (_, entry)) in sorted_sections.iter().enumerate() {
        if name_idx >= new_names.len() {
            break;
        }
        let section = &entry.table;
        if !is_section_writable(section) || section.virtual_size >= section.size_of_raw_data {
            continue;
        }

        let mut padding_start = section.virtual_size;
        let padding_end = section.size_of_raw_data;

        let next_va_limit = sorted_sections
            .get(si + 1)
            .map(|(_, s)| s.table.virtual_address - section.virtual_address)
            .unwrap_or(u32::MAX);

        while name_idx < new_names.len() {
            let name_with_null = new_names[name_idx].len() + 1;
            let space_by_raw = padding_end.saturating_sub(padding_start) as usize;
            let space_by_va = next_va_limit.saturating_sub(padding_start) as usize;
            let available = space_by_raw.min(space_by_va);

            if name_with_null > available {
                break;
            }

            let file_off = section.pointer_to_raw_data as usize + padding_start as usize;
            let rva = section.virtual_address + padding_start;
            slots.push((file_off, rva));

            padding_start += name_with_null as u32;
            name_idx += 1;
        }
    }

    if name_idx == new_names.len() {
        Some(slots)
    } else {
        None
    }
}

// -- PE checksum --

/// Calculate the PE checksum.
///
/// Algorithm: zero the checksum field, sum all u16 words with carry folding,
/// then add the file length.
fn pe_checksum(data: &[u8], checksum_offset: usize) -> u32 {
    let mut sum: u64 = 0;
    let len = data.len();
    let mut i = 0;

    while i + 1 < len {
        if i == checksum_offset || i == checksum_offset + 2 {
            i += 2;
            continue;
        }
        let word = u16::from_le_bytes([data[i], data[i + 1]]) as u64;
        sum += word;
        sum = (sum & 0xFFFF) + (sum >> 16);
        i += 2;
    }
    if i < len {
        sum += data[i] as u64;
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum = (sum & 0xFFFF) + (sum >> 16);

    (sum as u32) + (len as u32)
}

fn round_up(size: u32, alignment: u32) -> u32 {
    if alignment == 0 {
        size
    } else {
        size.next_multiple_of(alignment)
    }
}

// -- Public API --

/// Replace imported DLL name references in a PE file.
///
/// For each `(old_name, new_name)` pair, finds the import directory entry
/// that references `old_name` (case-insensitive) and rewrites it to point
/// to `new_name`.
///
/// Strategy:
/// 1. Try to fit new names into existing section padding
/// 2. If not enough padding, append a new PE section named "dlvwhl"
/// 3. Update import table RVAs, fix headers and checksum
///
/// Also clears DependentLoadFlags and removes Authenticode signatures.
pub fn replace_needed(file_path: &Path, replacements: &[(&str, &str)]) -> Result<()> {
    if replacements.is_empty() {
        clear_dependent_load_flags(file_path)?;
        return Ok(());
    }

    let mut data = fs::read(file_path)?;

    // Remove Authenticode signature if it's an appended overlay
    let layout = parse_pe_layout(&data)?;
    remove_authenticode(&mut data, &layout)?;

    let imports = parse_imports(&data, &layout)?;

    // Build case-insensitive mapping: lowercase old name → (new_name bytes, import ref index)
    let mut to_replace: Vec<(usize, &[u8])> = Vec::new();
    for (old_name, new_name) in replacements {
        let old_lower = old_name.to_lowercase();
        for (i, imp) in imports.iter().enumerate() {
            if imp.dll_name.to_lowercase() == old_lower {
                to_replace.push((i, new_name.as_bytes()));
            }
        }
    }

    if to_replace.is_empty() {
        clear_dependent_load_flags_in_data(&mut data, &layout)?;
        fix_and_write(&mut data, &layout, file_path)?;
        return Ok(());
    }

    let new_name_bytes: Vec<&[u8]> = to_replace.iter().map(|(_, b)| *b).collect();

    if let Some(slots) = find_padding_slots(&layout.sections, &new_name_bytes) {
        for ((imp_idx, new_bytes), (file_off, rva)) in to_replace.iter().zip(slots.iter()) {
            let end = file_off + new_bytes.len();
            data[*file_off..end].copy_from_slice(new_bytes);
            data[end] = 0;
            write_u32_le(&mut data, imports[*imp_idx].name_field_offset, *rva);
        }
        update_section_virtual_sizes(&mut data, &layout, &new_name_bytes, &slots)?;
    } else {
        add_new_section_with_names(&mut data, &layout, &imports, &to_replace)?;
    }

    // Re-parse since we may have modified the file
    let layout = parse_pe_layout(&data)?;
    clear_dependent_load_flags_in_data(&mut data, &layout)?;
    clear_certificate_table(&mut data, &layout);

    fix_and_write(&mut data, &layout, file_path)
}

/// Clear the DependentLoadFlags field in a PE file's Load Config Directory.
///
/// When DependentLoadFlags is non-zero (e.g., LOAD_LIBRARY_SEARCH_SYSTEM32),
/// it restricts which directories Windows searches for dependent DLLs.
/// This can prevent `AddDllDirectory()` from working, breaking the
/// `os.add_dll_directory()` approach. Clearing it restores the default
/// search behavior.
pub fn clear_dependent_load_flags(file_path: &Path) -> Result<bool> {
    let mut data = fs::read(file_path)?;
    let layout = parse_pe_layout(&data)?;

    let cleared = clear_dependent_load_flags_in_data(&mut data, &layout)?;
    if cleared {
        fix_and_write(&mut data, &layout, file_path)?;
    }
    Ok(cleared)
}

// -- Internal helpers --

fn clear_dependent_load_flags_in_data(data: &mut [u8], layout: &PeLayout) -> Result<bool> {
    let Some((lc_rva, lc_size)) = get_data_dir(data, layout, DD_LOAD_CONFIG)? else {
        return Ok(false);
    };
    let sections: Vec<_> = layout.section_tables().cloned().collect();
    let Some(lc_offset) = rva_to_offset(lc_rva, &sections) else {
        return Ok(false);
    };

    // DependentLoadFlags offset within Load Config:
    // PE32+: 78, PE32: 54
    let dlf_offset_in_struct: usize = if layout.is_64bit { 78 } else { 54 };

    if (lc_size as usize) < dlf_offset_in_struct + 2 {
        return Ok(false);
    }
    let dlf_file_offset = lc_offset + dlf_offset_in_struct;
    if dlf_file_offset + 2 > data.len() {
        return Ok(false);
    }

    let flags = read_u16_le(data, dlf_file_offset)?;
    if flags != 0 {
        tracing::debug!("Clearing DependentLoadFlags={:#x}", flags);
        write_u16_le(data, dlf_file_offset, 0);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn clear_certificate_table(data: &mut [u8], layout: &PeLayout) {
    if DD_SECURITY < layout.num_data_dirs as usize {
        let off = layout.data_dirs_offset + DD_SECURITY * 8;
        if off + 8 <= data.len() {
            write_u32_le(data, off, 0);
            write_u32_le(data, off + 4, 0);
        }
    }
}

fn remove_authenticode(data: &mut Vec<u8>, layout: &PeLayout) -> Result<()> {
    // The security directory is special: it uses raw file offsets, not RVAs
    let Some((cert_file_offset, cert_size)) = get_data_dir(data, layout, DD_SECURITY)? else {
        return Ok(());
    };

    let pe_size = layout
        .section_tables()
        .map(|s| s.pointer_to_raw_data.saturating_add(s.size_of_raw_data) as usize)
        .max()
        .unwrap_or(0);

    let cert_start = round_up(pe_size as u32, 8) as usize;
    if cert_file_offset as usize == cert_start
        && cert_file_offset.saturating_add(cert_size) as usize == data.len()
    {
        data.truncate(pe_size);
    }
    Ok(())
}

fn update_section_virtual_sizes(
    data: &mut [u8],
    layout: &PeLayout,
    new_names: &[&[u8]],
    slots: &[(usize, u32)],
) -> Result<()> {
    for (name_bytes, &(_file_off, rva)) in new_names.iter().zip(slots) {
        let name_len = name_bytes.len() as u32 + 1;
        for entry in &layout.sections {
            if rva >= entry.table.virtual_address
                && rva < entry.table.virtual_address + entry.table.size_of_raw_data
            {
                let new_end = (rva - entry.table.virtual_address) + name_len;
                let current_vs = read_u32_le(data, entry.header_offset + 8)?;
                if new_end > current_vs {
                    write_u32_le(data, entry.header_offset + 8, new_end);
                }
                break;
            }
        }
    }
    Ok(())
}

fn add_new_section_with_names(
    data: &mut Vec<u8>,
    layout: &PeLayout,
    imports: &[ImportRef],
    to_replace: &[(usize, &[u8])],
) -> Result<()> {
    let mut section_data = Vec::new();
    let mut name_offsets = Vec::new();
    for (_, new_bytes) in to_replace {
        name_offsets.push(section_data.len());
        section_data.extend_from_slice(new_bytes);
        section_data.push(0);
    }

    let section_data_size = section_data.len() as u32;
    let section_data_padded = round_up(section_data_size, layout.file_alignment);

    let new_section_rva = layout
        .section_tables()
        .map(|s| round_up(s.virtual_address + s.virtual_size, layout.section_alignment))
        .max()
        .unwrap_or(0);

    let section_table_end =
        layout.section_table_offset + layout.sections.len() * SECTION_HEADER_SIZE;
    let size_of_headers = read_u32_le(data, layout.size_of_headers_offset)?;

    // Determine how much extra header space is needed for the new section header.
    // If there's a gap between the last section header and SizeOfHeaders, use it.
    // Otherwise, expand headers by FileAlignment bytes and shift all section data.
    let header_space_needed =
        if (size_of_headers as usize).saturating_sub(section_table_end) >= SECTION_HEADER_SIZE {
            0u32
        } else {
            round_up(SECTION_HEADER_SIZE as u32, layout.file_alignment)
        };

    let pe_data_end = layout
        .section_tables()
        .map(|s| s.pointer_to_raw_data + s.size_of_raw_data)
        .max()
        .unwrap_or(size_of_headers);

    // Check for non-Authenticode overlay (Authenticode was already removed
    // by remove_authenticode before we get here). Overlays contain data like
    // debug symbols that we cannot safely remove.
    if data.len() > pe_data_end as usize {
        bail!(
            "PE file contains a {}-byte overlay beyond section data that cannot be \
             safely removed. This is usually debug symbols or other appended data. \
             Strip the DLL with `strip -s` before building.",
            data.len() - pe_data_end as usize
        );
    }

    if header_space_needed > 0 {
        // Insert zero bytes at the section table end to make room for the new
        // section header. This shifts all section data forward, so we must
        // update every section's PointerToRawData.
        let extra = header_space_needed as usize;
        data.splice(section_table_end..section_table_end, vec![0u8; extra]);

        for entry in &layout.sections {
            let old_ptr = read_u32_le(data, entry.header_offset + 20)?;
            write_u32_le(
                data,
                entry.header_offset + 20,
                old_ptr + header_space_needed,
            );
        }

        write_u32_le(
            data,
            layout.size_of_headers_offset,
            size_of_headers + header_space_needed,
        );
    }

    // PointerToRawData for the new section accounts for any header expansion
    let new_section_file_offset = pe_data_end + header_space_needed;

    // Build the section header (40 bytes)
    let mut header = [0u8; SECTION_HEADER_SIZE];
    header[..6].copy_from_slice(b"dlvwhl");
    write_u32_le(&mut header, 8, section_data_size); // VirtualSize
    write_u32_le(&mut header, 12, new_section_rva); // VirtualAddress
    write_u32_le(&mut header, 16, section_data_padded); // SizeOfRawData
    write_u32_le(&mut header, 20, new_section_file_offset); // PointerToRawData
    write_u32_le(&mut header, 36, NEW_SECTION_CHARS); // Characteristics

    data[section_table_end..section_table_end + SECTION_HEADER_SIZE].copy_from_slice(&header);

    // Append section data + padding at the end of file
    data.resize(new_section_file_offset as usize, 0);
    data.extend_from_slice(&section_data);
    data.resize(
        new_section_file_offset as usize + section_data_padded as usize,
        0,
    );

    // Update PE headers
    let new_num_sections = layout.sections.len() as u16 + 1;
    write_u16_le(data, layout.num_sections_offset, new_num_sections);

    let new_size_of_image = round_up(
        new_section_rva + section_data_size,
        layout.section_alignment,
    );
    write_u32_le(data, layout.size_of_image_offset, new_size_of_image);

    let current_init_data = read_u32_le(data, layout.size_of_init_data_offset)?;
    write_u32_le(
        data,
        layout.size_of_init_data_offset,
        current_init_data + section_data_padded,
    );

    // Update import table RVAs. If we shifted headers, the import descriptor
    // file offsets moved too, so we must account for that.
    for ((imp_idx, _), name_off) in to_replace.iter().zip(name_offsets.iter()) {
        let new_rva = new_section_rva + *name_off as u32;
        let adjusted_offset = imports[*imp_idx].name_field_offset + header_space_needed as usize;
        write_u32_le(data, adjusted_offset, new_rva);
    }

    Ok(())
}

fn fix_and_write(data: &mut Vec<u8>, layout: &PeLayout, file_path: &Path) -> Result<()> {
    let old_checksum = read_u32_le(data, layout.checksum_offset)?;
    let new_checksum = pe_checksum(data, layout.checksum_offset);
    if old_checksum != 0 {
        write_u32_le(data, layout.checksum_offset, new_checksum);
    }

    fs::write(file_path, data.as_slice())
        .with_context(|| format!("Failed to write patched PE file: {}", file_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid PE32+ file with an import table.
    fn make_test_pe(dll_names: &[&str]) -> Vec<u8> {
        let pe_offset: u32 = 0x80;
        let coff_offset = pe_offset as usize + 4;
        let opt_hdr_offset = coff_offset + 20;

        let opt_hdr_fixed_size = 112;
        let num_data_dirs: u32 = 16;
        let data_dirs_size = (num_data_dirs * 8) as usize;
        let opt_hdr_size = opt_hdr_fixed_size + data_dirs_size;
        let section_table_offset = opt_hdr_offset + opt_hdr_size;
        let num_sections = 1;
        let headers_end = section_table_offset + num_sections * SECTION_HEADER_SIZE;
        let headers_size = round_up(headers_end as u32, 0x200);

        let section_rva: u32 = 0x1000;
        let section_file_offset = headers_size;
        let section_alignment: u32 = 0x1000;
        let file_alignment: u32 = 0x200;

        let import_dir_rva = section_rva;
        let import_dir_size = (dll_names.len() + 1) * SIZEOF_IMPORT_DIRECTORY_ENTRY;

        let names_start = import_dir_size;
        let mut name_offsets = Vec::new();
        let mut current_offset = names_start;
        for name in dll_names {
            name_offsets.push(current_offset);
            current_offset += name.len() + 1;
        }
        let section_virtual_size = current_offset as u32;
        let section_raw_size = round_up(section_virtual_size + 64, file_alignment);

        let file_size = section_file_offset + section_raw_size;
        let mut data = vec![0u8; file_size as usize];

        // DOS header
        data[0] = b'M';
        data[1] = b'Z';
        write_u32_le(&mut data, 0x3C, pe_offset);

        // PE signature
        data[pe_offset as usize..pe_offset as usize + 4].copy_from_slice(b"PE\0\0");

        // COFF header
        write_u16_le(&mut data, coff_offset, 0x8664); // Machine: AMD64
        write_u16_le(&mut data, coff_offset + 2, num_sections as u16);
        write_u16_le(&mut data, coff_offset + 16, opt_hdr_size as u16);

        // Optional header
        write_u16_le(&mut data, opt_hdr_offset, 0x20B); // PE32+ magic
        data[opt_hdr_offset + 2] = 14; // MajorLinkerVersion
        data[opt_hdr_offset + 3] = 30; // MinorLinkerVersion

        let size_of_image = round_up(section_rva + section_virtual_size, section_alignment);
        write_u32_le(&mut data, opt_hdr_offset + 56, size_of_image);
        write_u32_le(&mut data, opt_hdr_offset + 60, headers_size);
        write_u32_le(&mut data, opt_hdr_offset + 64, 0); // CheckSum

        write_u32_le(&mut data, opt_hdr_offset + 32, section_alignment);
        write_u32_le(&mut data, opt_hdr_offset + 36, file_alignment);

        let data_dirs_offset = opt_hdr_offset + opt_hdr_fixed_size;
        write_u32_le(&mut data, data_dirs_offset - 4, num_data_dirs);

        // Data directory[1] = Import Table
        write_u32_le(&mut data, data_dirs_offset + DD_IMPORT * 8, import_dir_rva);
        write_u32_le(
            &mut data,
            data_dirs_offset + DD_IMPORT * 8 + 4,
            import_dir_size as u32,
        );

        // Section header (.rdata)
        let sh_off = section_table_offset;
        data[sh_off..sh_off + 6].copy_from_slice(b".rdata");
        write_u32_le(&mut data, sh_off + 8, section_virtual_size);
        write_u32_le(&mut data, sh_off + 12, section_rva);
        write_u32_le(&mut data, sh_off + 16, section_raw_size);
        write_u32_le(&mut data, sh_off + 20, section_file_offset);
        write_u32_le(
            &mut data,
            sh_off + 36,
            IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ,
        );

        // Write import directory entries
        let section_data_offset = section_file_offset as usize;
        for (i, _name) in dll_names.iter().enumerate() {
            let entry_offset = section_data_offset + i * SIZEOF_IMPORT_DIRECTORY_ENTRY;
            let name_rva = section_rva + name_offsets[i] as u32;
            write_u32_le(&mut data, entry_offset + IMPORT_DESC_NAME_OFFSET, name_rva);
        }

        // Write DLL name strings
        for (i, name) in dll_names.iter().enumerate() {
            let str_offset = section_data_offset + name_offsets[i];
            data[str_offset..str_offset + name.len()].copy_from_slice(name.as_bytes());
            data[str_offset + name.len()] = 0;
        }

        data
    }

    #[test]
    fn test_pe_checksum() {
        let data = vec![0u8; 512];
        let cs = pe_checksum(&data, 200);
        assert_eq!(cs, 512);
    }

    #[test]
    fn test_rva_to_offset() {
        let mut section = SectionTable::default();
        section.virtual_size = 100;
        section.virtual_address = 0x1000;
        section.size_of_raw_data = 200;
        section.pointer_to_raw_data = 0x400;
        let sections = vec![section];

        assert_eq!(rva_to_offset(0x1000, &sections), Some(0x400));
        assert_eq!(rva_to_offset(0x1050, &sections), Some(0x450));
        assert_eq!(rva_to_offset(0x2000, &sections), None);
    }

    #[test]
    fn test_rva_to_offset_bss_section() {
        // BSS section where virtual_size > size_of_raw_data
        let mut section = SectionTable::default();
        section.virtual_size = 0x1000;
        section.virtual_address = 0x2000;
        section.size_of_raw_data = 0x200;
        section.pointer_to_raw_data = 0x600;
        let sections = vec![section];

        // Within raw data — should resolve
        assert_eq!(rva_to_offset(0x2000, &sections), Some(0x600));
        assert_eq!(rva_to_offset(0x2100, &sections), Some(0x700));
        // Within virtual range but beyond raw data (BSS zero-fill) — no file backing
        assert_eq!(rva_to_offset(0x2500, &sections), None);
        // Beyond the section entirely
        assert_eq!(rva_to_offset(0x4000, &sections), None);
    }

    #[test]
    fn test_parse_pe() {
        let data = make_test_pe(&["kernel32.dll", "foo.dll"]);
        let layout = parse_pe_layout(&data).unwrap();

        assert!(layout.is_64bit);
        assert_eq!(layout.sections.len(), 1);
        assert!(layout.file_alignment > 0);
        assert!(layout.section_alignment > 0);
    }

    #[test]
    fn test_parse_imports() {
        let data = make_test_pe(&["kernel32.dll", "foo.dll"]);
        let layout = parse_pe_layout(&data).unwrap();
        let imports = parse_imports(&data, &layout).unwrap();

        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].dll_name, "kernel32.dll");
        assert_eq!(imports[1].dll_name, "foo.dll");
    }

    #[test]
    fn test_replace_needed_with_padding() {
        let data = make_test_pe(&["kernel32.dll", "foo.dll"]);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), &data).unwrap();

        replace_needed(tmp.path(), &[("foo.dll", "foo-ab12cd34.dll")]).unwrap();

        let patched = fs::read(tmp.path()).unwrap();
        let layout = parse_pe_layout(&patched).unwrap();
        let imports = parse_imports(&patched, &layout).unwrap();

        assert_eq!(imports[0].dll_name, "kernel32.dll");
        assert_eq!(imports[1].dll_name, "foo-ab12cd34.dll");
    }
}
