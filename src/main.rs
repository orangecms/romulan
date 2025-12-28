// SPDX-License-Identifier: MIT

use clap::Parser;
use romulan::amd;
use romulan::intel::{self, section, volume};
use romulan::intel::{BiosFile, BiosSection, BiosSections, BiosVolume, BiosVolumes};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::{fs, io, mem, thread};
use uefi::guid::SECTION_LZMA_COMPRESS_GUID;

pub mod diff_amd;
use diff_amd::{
    diff_bios, diff_efs, diff_psp, print_bios_dir_from_addr, print_psp_dirs, BIOS_DIR_NAMES,
};

const K: usize = 1024;

/// Romulan the ROM analysis tool
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Print
    #[arg(required = false, short, long)]
    print: bool,

    /// Print verbosely
    #[arg(required = false, short, long)]
    verbose: bool,

    /// Print as JSON
    #[arg(required = false, short, long)]
    json: bool,

    /// Dump files
    #[arg(required = false, short, long)]
    dump: bool,

    /// File to read
    #[arg(index = 1)]
    file1: String,

    /// File to diff
    #[arg(index = 2)]
    file2: Option<String>,
}

fn dump_lzma(compressed_data: &[u8], padding: &str) {
    // For some reason, xz2 does not work with this data
    let mut child = Command::new("xz")
        .arg("--decompress")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let data = {
        let mut stdout = child.stdout.take().unwrap();
        let read_thread = thread::spawn(move || -> io::Result<Vec<u8>> {
            let mut data = Vec::<u8>::new();
            stdout.read_to_end(&mut data)?;
            Ok(data)
        });

        {
            let mut stdin = child.stdin.take().unwrap();
            let _write_result = stdin.write_all(compressed_data);
        }

        read_thread.join().unwrap().unwrap()
    };

    let status = child.wait().unwrap();
    if status.success() {
        let len = data.len() / K;
        println!("{padding}Decompressed: {len} K");

        for section in BiosSections::new(&data) {
            dump_section(&section, &format!("{padding}    "));
        }
    } else {
        println!("{padding}Error: {status}");
    }
}

fn dump_guid_defined(section_data: &[u8], padding: &str) {
    let header = plain::from_bytes::<section::GuidDefined>(section_data).unwrap();
    let data_offset = header.data_offset;
    let data = &section_data[(data_offset as usize)..];
    let guid = header.guid;
    let len = data.len() / K;
    println!("{padding}  {guid}: {len} K");

    #[allow(clippy::single_match)]
    match guid {
        SECTION_LZMA_COMPRESS_GUID => {
            let compressed_data = &section_data[mem::size_of::<section::GuidDefined>()..];
            dump_lzma(compressed_data, &format!("{padding}    "));
        }
        _ => (),
    }
}

fn dump_section(section: &BiosSection, padding: &str) {
    let header = section.header();
    let kind = header.kind();
    let data = section.data();
    let len = data.len() / K;
    println!("{padding}{kind:?}:  {len} K");

    match kind {
        section::HeaderKind::GuidDefined => {
            dump_guid_defined(data, &format!("{padding}    "));
        }
        section::HeaderKind::VolumeImage => {
            for volume in BiosVolumes::new(data) {
                dump_volume(&volume, &format!("{padding}    "));
            }
        }
        _ => (),
    }
}

fn dump_file(file: &BiosFile, polarity: bool, padding: &str) {
    let header = file.header();
    let guid = header.guid;
    let data = file.data();
    let len = data.len() / K;
    let kind = header.kind();
    let attributes = header.attributes();
    let alignment = header.alignment();
    let state = header.state(polarity);
    println!("{padding}{guid}: {len} K");
    println!("{padding}  Kind: {kind:?}");
    println!("{padding}  Attrib: {attributes:?}");
    println!("{padding}  Align: {alignment}");
    println!("{padding}  State: {state:?}");

    if header.sectioned() {
        for section in file.sections() {
            dump_section(&section, &format!("{padding}    "));
        }
    }
}

fn dump_volume(volume: &BiosVolume, padding: &str) {
    let header = volume.header();
    let guid = header.guid;
    let header_len = header.header_length;
    let len = volume.data().len() / K;
    let attributes = header.attributes();
    println!("{padding}{guid}: {header_len}, {len} K");
    println!("{padding}  Attrib: {attributes:?}");

    let polarity = attributes.contains(volume::Attributes::ERASE_POLARITY);
    for file in volume.files() {
        dump_file(&file, polarity, &format!("{padding}    "));
    }
}

fn print_intel(rom: &intel::Rom, _print_json: bool, verbose: bool) {
    let hap = match rom.high_assurance_platform() {
        Ok(r) => {
            if r {
                "set".to_string()
            } else {
                "not set".to_string()
            }
        }
        Err(e) => format!("unknown: {e}"),
    };
    println!("  HAP: {hap}");

    if let Ok(bios) = rom.bios() {
        let len = bios.data().len() / K;
        println!("  BIOS: {len} K");
        if verbose {
            for volume in bios.volumes() {
                dump_volume(&volume, "    ");
            }
        }
    } else {
        println!("  BIOS: None");
    }

    if let Ok(me) = rom.me() {
        let len = me.data().len() / K;
        println!("  ME: {len} K");
        let v = me.version().unwrap_or("Unknown".to_string());
        println!("    Version: {v}");
        let d = me.data();
        /*
        match me_fs_rs::parse(d) {
            Ok(fpt) => {
                println!("{:#08?}", fpt.header);
            }
            Err(e) => println!("ME parser: {e}"),
        }
        */
    } else {
        println!("  ME: None");
    }
}

fn print_amd(rom: &amd::Rom, print_json: bool) {
    if print_json {
        // TODO: Wrap in EFS: {} or something
        if let Ok(j) = serde_json::to_string_pretty(&rom.efs()) {
            println!("{j}");
        }
    } else {
        let data = rom.data();
        let efs = rom.efs();

        println!();
        println!("{efs}");
        println!();
        println!(": Directories :");
        println!();
        println!(":: BIOS ::");
        let dirs = [
            efs.bios_17_00_0f,
            efs.bios_17_10_1f,
            efs.bios_17_30_3f_19_00_0f,
            efs.bios_17_60,
        ];
        for (i, dir) in dirs.iter().enumerate() {
            println!();
            println!("=== {} ===", BIOS_DIR_NAMES[i]);
            if *dir != 0x0000_0000 && *dir != 0xffff_ffff {
                print_bios_dir_from_addr(*dir as usize, data);
            } else {
                println!();
                println!("no BIOS dir @ {dir:08x}");
            }
        }
        match rom.psp_legacy() {
            Ok(psp) => {
                println!();
                let b = efs.psp_legacy as usize;
                println!("# legacy PSP {psp} @ {b:08x}");
                print_psp_dirs(&psp, data);
            }
            Err(e) => {
                println!();
                println!("# legacy PSP: {e}");
            }
        }
        match rom.psp_17_00() {
            Ok(psp) => {
                println!();
                let b = efs.psp_17_00 as usize;
                println!("# Fam 17 PSP {psp} @ {b:08x}");
                print_psp_dirs(&psp, data);
            }
            Err(e) => {
                println!();
                println!("# Fam 17 PSP: {e}");
            }
        }
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let file1 = args.file1;
    let data1 = fs::read(file1.clone()).unwrap();
    let verbose = args.verbose;
    let do_print = args.print || verbose;
    let print_json = args.json;

    if let Some(file2) = args.file2 {
        println!("Diffing {file1} vs {file2}");
        let data2 = fs::read(file2).unwrap();
        let rom1 = amd::Rom::new(&data1).unwrap();
        let rom2 = amd::Rom::new(&data2).unwrap();
        if verbose {
            println!("data1: {}", data1.len());
            println!("data2: {}", data2.len());
        }
        if verbose {
            println!(": Image 1 :");
            print_amd(&rom1, print_json);
            println!(": Image 2 :");
            print_amd(&rom2, print_json);
        }
        println!();
        let efs1 = rom1.efs();
        let efs2 = rom2.efs();
        diff_efs(&efs1, &efs2);
        println!();
        diff_psp(&rom1, &rom2, verbose);
        println!();
        diff_bios(&rom1, &rom2, verbose);
    } else {
        println!("Scanning {file1}");
        match intel::Rom::new(&data1) {
            Ok(rom) => {
                if do_print {
                    println!("Intel inside");
                    print_intel(&rom, print_json, verbose);
                }
            }
            Err(e) => println!("No Intel inside: {e}"),
        }
        match amd::Rom::new(&data1) {
            Ok(rom) => {
                println!("AMD inside");
                if do_print {
                    print_amd(&rom, print_json);
                }
            }
            Err(e) => println!("No AMD inside: {e}"),
        }
    }

    Ok(())
}
