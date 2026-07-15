use std::{
    fs,
    io,
    sync::{Arc, mpsc, Mutex},
    thread,
};

use crate::image::WallpaperFile;

const MAX_IO_THREADS: usize = 2;

pub struct WallpaperData {
    pub file: WallpaperFile,
    pub data: io::Result<Vec<u8>>,
}

pub fn read_all(files: Vec<WallpaperFile>) -> mpsc::Receiver<WallpaperData> {
    let count = files.len().min(MAX_IO_THREADS);
    let tasks = Arc::new(Mutex::new(files));
    let (sender, receiver) = mpsc::sync_channel(0);
    for i in 0..count {
        let tasks = Arc::clone(&tasks);
        let sender = sender.clone();
        thread::Builder::new()
            .name(format!("file_io_{i}"))
            .spawn(|| run(tasks, sender))
            .unwrap();
    }
    receiver
}

fn run(
    tasks: Arc<Mutex<Vec<WallpaperFile>>>,
    sender: mpsc::SyncSender<WallpaperData>,
) {
    while let Some(file) = tasks.lock().unwrap().pop() {
        let data = fs::read(&file.path);
        sender.send(WallpaperData { file, data }).unwrap();
    }
}
