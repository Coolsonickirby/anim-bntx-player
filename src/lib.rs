#![feature(proc_macro_hygiene)]

use binrw::BinRead;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::thread;
use std::time::Duration;
use std::path::*;
use arcropolis_api::*;
use walkdir::WalkDir;


static mut HASH_TO_PATH: Lazy<HashMap<u64, String>> = Lazy::new(|| HashMap::new());

#[derive(BinRead)]
#[br(magic = b"AnimBNTX")]
struct AnimationBNTX {
    frame_count: u32,
    loop_animation: u32,
    loop_count: i32,
    starting_frame_loop: u32,
    ending_frame_loop: u32,
    relocation_table_size: u32,
    image_data_count: u32,
    image_data_size: u32,
    frame_rate: f32,

    #[br(count = frame_count)]
    frame_datas: Vec<FrameData>,

    #[br(count = 0x1000)]
    bntx_template_header: Vec<u8>,
    
    #[br(count = relocation_table_size)]
    bntx_template_footer: Vec<u8>,
    
    #[br(count = image_data_count * image_data_size)]
    image_datas: Vec<u8>
}

impl AnimationBNTX {
    pub fn image_data_at_index(&self, index: usize) -> &[u8] {
        &self.image_datas[index * self.image_data_size as usize..(index + 1) * self.image_data_size as usize]
    }
}

#[derive(BinRead, Debug)]
struct FrameData {
    keyframe_num: u32,
    image_index: u32
}

const SCAN_DIR: &str = "sd:/ultimate/mods";

#[repr(C)]
#[derive(Copy, Clone)]
pub enum Event {
    ArcFilesystemMounted,
    ModFilesystemMounted,
}

pub type EventCallbackFn = extern "C" fn(Event);

extern "C" {
    fn arcrop_register_event_callback(ty: Event, callback: EventCallbackFn);
    fn auto_refresh_bntx(hash: u64, replace: *mut u8, size: usize) -> bool;
}

pub extern "C" fn ArcFileReady(_event: Event) {
    unsafe {
        scan_dirs(Path::new(&SCAN_DIR));
    }
}

unsafe fn scan_dirs(starting_path: &Path){
    match std::fs::read_dir(starting_path) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.unwrap();
                let entry_str = format!("{}", entry.path().display());

                if !is_mod_enabled(Hash40::from(entry_str.as_str()).as_u64()) {
                    continue;
                }

                look_for_animbntxs(&entry_str);
            }
        },
        Err(err) => println!("[anim-bntx-player::scan_dirs] Error reading dir: {:?}", err)
    }
}

unsafe fn look_for_animbntxs(mod_path: &str){
    let mut paths_to_check: Vec<String> = Vec::new();
    for entry in WalkDir::new(mod_path)
        .into_iter()
    {
        let entry_str = format!("{}", entry.unwrap().path().display());
        if entry_str.contains(".animbntx") {
            paths_to_check.push(entry_str);
        }
    }

    for path in paths_to_check {
        let arc_path = &format!("{}", path)[mod_path.len() + 1..].replace(";", ":").replace(".animbntx", ".bntx");
        let hash = hash40(&arc_path).as_u64();
        HASH_TO_PATH.insert(hash.clone(), path);
        setup_animbntx_callback(&hash);
    }
}

unsafe fn setup_animbntx_callback(hash: &u64){
    let file_path = HASH_TO_PATH.get(hash).unwrap();
    match std::fs::read(&file_path){
        Ok(data) => {
            let mut data = std::io::Cursor::new(data);
            let animation_bntx_data = AnimationBNTX::read_le(&mut data).unwrap();
            let bntx_callback_size = (animation_bntx_data.image_data_size as usize) + 0x1000 + animation_bntx_data.bntx_template_footer.len() as usize;
            
            bntx_callback::install(hash.clone(), bntx_callback_size);
        },
        Err(err) => println!("[anim-bntx-player::setup_animbntx_callback] Error reading file: {} - {:?}", file_path, err)
    }
}

#[arc_callback]
fn bntx_callback(hash: u64, bntx_data: &mut [u8]) -> Option<usize> {
    unsafe {
        println!("Callback ran for {:#x}", hash);
        
        let file_path = HASH_TO_PATH.get(&hash).unwrap();
        match std::fs::read(&file_path){
            Ok(data) => {
                let mut data = std::io::Cursor::new(data);
                let animation_bntx_data = AnimationBNTX::read_le(&mut data).unwrap();
                let bntx_callback_size = (animation_bntx_data.image_data_size as usize) + 0x1000 + animation_bntx_data.bntx_template_footer.len() as usize;
                
                let mut callback_data_vec: Vec<u8> = Vec::new();
    
                callback_data_vec.extend(animation_bntx_data.bntx_template_header.clone());
                callback_data_vec.extend(animation_bntx_data.image_data_at_index(0));
                callback_data_vec.extend(animation_bntx_data.bntx_template_footer.clone());

                std::thread::spawn(move || {
                    let mut current_frame = 0;
                    let mut current_loop_count = 0;
                    let sleep_duration = 1000.0 / animation_bntx_data.frame_rate;
                    let loop_animation = animation_bntx_data.loop_animation == 1;
                    loop {
                        if !is_file_loaded(hash.clone()){
                            break;
                        }

                        if current_frame >= animation_bntx_data.ending_frame_loop {
                            if loop_animation {
                                if current_loop_count < animation_bntx_data.loop_count || animation_bntx_data.loop_count == -1 {
                                    current_frame = animation_bntx_data.starting_frame_loop;
                                    current_loop_count += 1;
                                }
                            }
                        }
                        
                        if current_frame >= animation_bntx_data.frame_count {
                            if !loop_animation {
                                current_frame = animation_bntx_data.ending_frame_loop;
                            } else {
                                current_frame = animation_bntx_data.starting_frame_loop;
                            }
                        }

                        let frame_data = &animation_bntx_data.frame_datas[current_frame as usize];

                        let image_slice = animation_bntx_data.image_data_at_index(frame_data.image_index as usize);

                        let mut data_vec: Vec<u8> = Vec::new();
                        data_vec.extend(animation_bntx_data.bntx_template_header.clone());
                        data_vec.extend(image_slice);
                        data_vec.extend(animation_bntx_data.bntx_template_footer.clone());
                        let data_slice = data_vec.as_slice();

                        auto_refresh_bntx(
                            hash.clone(),
                            data_slice.as_ptr() as *mut u8,
                            data_slice.len(),
                        );

                        current_frame += 1;
                        thread::sleep(Duration::from_millis(sleep_duration as u64));
                    }
                });

                bntx_data[..callback_data_vec.len()].copy_from_slice(&callback_data_vec.as_slice());
                return Some(callback_data_vec.len());
            },
            Err(err) => println!("[anim-bntx-player::bntx_callback] Error reading file: {} - {:?}", file_path, err)
        }
    
        None
    }
}

#[skyline::main(name = "anim-bntx-player")]
pub fn main() {
    unsafe {
        arcrop_register_event_callback(Event::ArcFilesystemMounted, ArcFileReady);
    }
}
