#![feature(proc_macro_hygiene)]

use binrw::BinRead;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::thread;
use std::time::Duration;
use std::path::*;
use arcropolis_api::*;
use walkdir::WalkDir;


// HASH_TO_PATH -> used to load AnimBNTX in callback
// FPS_TO_THREAD -> each fps has a thread associated with it that will play all files in that fps
// HASHS_IN_FPS -> all hashes that belong to a certain fps
// HASH_TO_ANIM_BNTX -> each loaded hash and it's respective AnimationBNTX
// HASH_TO_CURRENT_FRAME -> each hash's current frame
// HASH_TO_CURRENT_LOOP_COUNT -> each hash's current loop count
// GROUP_TO_CURRENT_FRAME -> each (animationbntx group num << 16) + (fps_duration << 16) + loop_animation current frame (used to keep multiple animations on the same frames)


static mut HASH_TO_PATH: Lazy<HashMap<u64, String>> = Lazy::new(|| HashMap::new());
static mut FPS_TO_THREAD: Lazy<HashMap<u64, thread::JoinHandle<()>>> = Lazy::new(|| HashMap::new());
static mut HASHS_IN_FPS: Lazy<HashMap<u64, Vec<u64>>> = Lazy::new(|| HashMap::new());
static mut HASH_TO_ANIM_BNTX: Lazy<HashMap<u64, AnimationBNTX>> = Lazy::new(|| HashMap::new());
static mut HASH_TO_CURRENT_FRAME: Lazy<HashMap<u64, u32>> = Lazy::new(|| HashMap::new());
static mut HASH_TO_CURRENT_LOOP_COUNT: Lazy<HashMap<u64, i32>> = Lazy::new(|| HashMap::new());
static mut GROUP_TO_CURRENT_FRAME: Lazy<HashMap<u64, u32>> = Lazy::new(|| HashMap::new());
static mut GROUP_TO_CURRENT_LOOP_COUNT: Lazy<HashMap<u64, i32>> = Lazy::new(|| HashMap::new());
static mut GROUPS_IN_FPS: Lazy<HashMap<u64, Vec<u64>>> = Lazy::new(|| HashMap::new());
static mut GROUP_LOOP_ANIMATION: Lazy<HashMap<u64, bool>> = Lazy::new(|| HashMap::new());

#[derive(BinRead)]
#[br(magic = b"AnimBNTX")]
struct AnimationBNTX {
    version_major: u32,
    version_minor: u32,
    group_number: u32,
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
                let fps_duration = (1000.0 / animation_bntx_data.frame_rate) as u64;
                let loop_anim = animation_bntx_data.loop_animation;
                let group_id = (animation_bntx_data.group_number << 16) as u64 + (fps_duration << 16) + loop_anim as u64;
                
                let mut callback_data_vec: Vec<u8> = Vec::new();
    
                callback_data_vec.extend(animation_bntx_data.bntx_template_header.clone());
                callback_data_vec.extend(animation_bntx_data.image_data_at_index(0));
                callback_data_vec.extend(animation_bntx_data.bntx_template_footer.clone());

                if !HASHS_IN_FPS.contains_key(&fps_duration) {
                    HASHS_IN_FPS.insert(fps_duration.clone(), Vec::new());
                }

                HASHS_IN_FPS.get_mut(&fps_duration).unwrap().push(hash);
                HASH_TO_ANIM_BNTX.insert(hash.clone(), animation_bntx_data);
                HASH_TO_CURRENT_FRAME.insert(hash.clone(), 0);
                HASH_TO_CURRENT_LOOP_COUNT.insert(hash.clone(), 0);
                
                if !GROUPS_IN_FPS.contains_key(&fps_duration) {
                    GROUPS_IN_FPS.insert(fps_duration.clone(), Vec::new());
                }

                GROUPS_IN_FPS.get_mut(&fps_duration).unwrap().push(group_id.clone());
                GROUP_TO_CURRENT_FRAME.insert(group_id.clone(), 0);
                GROUP_TO_CURRENT_LOOP_COUNT.insert(group_id.clone(), 0);


                if !GROUP_LOOP_ANIMATION.contains_key(&group_id) {
                    GROUP_LOOP_ANIMATION.insert(group_id.clone(), loop_anim == 1);
                }

                if !FPS_TO_THREAD.contains_key(&fps_duration) {
                    let thread_data = std::thread::spawn(move || {
                        loop {
                            let hashes = HASHS_IN_FPS.get(&fps_duration).unwrap();

                            if hashes.len() == 0 {
                                // Remove thread from fps_to_thread and associated groups
                                FPS_TO_THREAD.remove(&fps_duration);
                                for group in GROUPS_IN_FPS.get(&fps_duration).unwrap() {
                                    GROUP_TO_CURRENT_FRAME.remove(&group);
                                    GROUP_TO_CURRENT_LOOP_COUNT.remove(&group);
                                    GROUP_LOOP_ANIMATION.remove(&group);
                                }
                                GROUPS_IN_FPS.remove(&fps_duration);
                                break;
                            }

                            for hash in hashes {
                                let anim_bntx = HASH_TO_ANIM_BNTX.get(&hash).unwrap();

                                let mut current_frame = {
                                    if anim_bntx.group_number == 0 {
                                        HASH_TO_CURRENT_FRAME.get_mut(&hash).unwrap()
                                    } else {
                                        GROUP_TO_CURRENT_FRAME.get_mut(&group_id).unwrap()
                                    }
                                };

                                let mut current_loop_count = {
                                    if anim_bntx.group_number == 0 {
                                        HASH_TO_CURRENT_LOOP_COUNT.get_mut(&hash).unwrap()
                                    } else {
                                        GROUP_TO_CURRENT_LOOP_COUNT.get_mut(&group_id).unwrap()
                                    }
                                };

                                let loop_animation = {
                                    if anim_bntx.group_number == 0 {
                                        anim_bntx.loop_animation == 1
                                    } else {
                                        *GROUP_LOOP_ANIMATION.get(&group_id).unwrap()
                                    }
                                };

                                if !is_file_loaded(hash.clone()){
                                    // Remove from respective hashes
                                    HASH_TO_ANIM_BNTX.remove(&hash);
                                    HASH_TO_CURRENT_FRAME.remove(&hash);
                                    HASH_TO_CURRENT_LOOP_COUNT.remove(&hash);
                                    
                                    let index = hashes.iter().position(|x| *x == *hash).unwrap();
                                    HASHS_IN_FPS.get_mut(&fps_duration).unwrap().remove(index);
                                    
                                    continue;
                                }

                                if *current_frame >= anim_bntx.ending_frame_loop {
                                    if loop_animation {
                                        if *current_loop_count < anim_bntx.loop_count || anim_bntx.loop_count == -1 {
                                            *current_frame = anim_bntx.starting_frame_loop;
                                            *current_loop_count += 1;
                                        }
                                    }
                                }

                                if *current_frame >= anim_bntx.frame_count {
                                    if !loop_animation {
                                        *current_frame = anim_bntx.ending_frame_loop;
                                    } else {
                                        *current_frame = anim_bntx.starting_frame_loop;
                                    }
                                }

                                let frame_data = &anim_bntx.frame_datas[*current_frame as usize];
                                let image_slice = anim_bntx.image_data_at_index(frame_data.image_index as usize);
        
                                let mut data_vec: Vec<u8> = Vec::new();
                                data_vec.extend(anim_bntx.bntx_template_header.clone());
                                data_vec.extend(image_slice);
                                data_vec.extend(anim_bntx.bntx_template_footer.clone());
                                let data_slice = data_vec.as_slice();
        
                                auto_refresh_bntx(
                                    hash.clone(),
                                    data_slice.as_ptr() as *mut u8,
                                    data_slice.len(),
                                );
        
                                if anim_bntx.group_number != 0 {
                                    *current_frame += 1;
                                }
                            }

                            for group in GROUPS_IN_FPS.get(&fps_duration).unwrap() {
                                *GROUP_TO_CURRENT_FRAME.get_mut(&group).unwrap() += 1;
                            }

                            thread::sleep(Duration::from_millis(fps_duration as u64));
                        }
                    });

                    FPS_TO_THREAD.insert(fps_duration, thread_data);
                }



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
