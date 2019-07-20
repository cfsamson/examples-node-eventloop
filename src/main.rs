/// Think of this function as the javascript program you have written which is then
/// run by the runtime
fn javascript() {
    println!("Thread: {}. Initiating read of test.txt", current());
    Fs::read("test.txt", |result| {
        // this is easier when dealing with javascript since you cast it to the relevant type
        // and there are no more checks...
        let text = result.to_string().unwrap();
        let len = text.len();
        println!("Thread: {}. First count: {} characters.", current(), len);

        println!("Thread: {}. I want to encrypt something.", current());
        Crypto::encrypt(text.len(), |result| {
            let n = result.to_int().unwrap();
            println!("Thread: {}. \"Encrypted\" number is: {}", current(), n);
        })
    });

    // let's read the file again and display the text
    println!(
        "Thread: {}. I want to read test.txt a second time",
        current()
    );
    Fs::read("test.txt", |result| {
        let text = result.to_string().unwrap();
        let len = text.len();
        println!("Thread: {}. Second count: {} characters.", current(), len);

        // aaand one more time but not in parallell.
        println!(
            "Thread: {}. I want to read test.txt a third time and then print the text",
            current()
        );
        Fs::read("test.txt", |result| {
            let text = result.to_string().unwrap();
            println!(
                "Thread: {}. The file contains the following text:\n\n\"{}\"\n",
                current(),
                text
            );
        });
    });

    Io::timeout(2500, |res| {
        println!("Timer timed out");
    });
}

fn current() -> String {
    thread::current().name().unwrap().to_string()
}

fn main() {
    let mut rt = Runtime::new();
    rt.run(javascript);
}

// ===== THIS IS OUR "NODE LIBRARY" =====
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

static mut RUNTIME: usize = 0;

type Callback = Box<Fn(Js)>;

struct Event {
    task: Box<Fn() -> Js + Send + 'static>,
    callback_id: usize,
    kind: EventKind,
}

enum EventKind {
    FileRead,
    Encrypt,
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use EventKind::*;
        match self {
            FileRead => write!(f, "File read"),
            Encrypt => write!(f, "Encrypt"),
        }
    }
}

#[derive(Debug)]
enum Js {
    Undefined,
    String(String),
    Int(usize),
}

impl Js {
    fn to_string(self) -> Option<String> {
        match self {
            Js::String(s) => Some(s),
            _ => None,
        }
    }

    fn to_int(self) -> Option<usize> {
        match self {
            Js::Int(n) => Some(n),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct NodeThread {
    handle: JoinHandle<()>,
    sender: Sender<Event>,
}

struct Runtime {
    thread_pool: Box<[NodeThread]>,
    available: Vec<usize>,
    callback_queue: Vec<Callback>,
    refs: usize,
    threadp_reciever: Receiver<(usize, usize, Js)>,
    epoll_reciever: Receiver<usize>,
    epoll_queue: i32,
    epoll_starer: Sender<()>,
}

impl Runtime {
    fn new() -> Self {
        let (threadp_sender, threadp_reciever) = channel::<(usize, usize, Js)>();
        let mut threads = Vec::with_capacity(4);

        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let threadp_sender = threadp_sender.clone();
            let handle = thread::Builder::new()
                .name(i.to_string())
                .spawn(move || {
                    while let Ok(event) = evt_reciever.recv() {
                        println!(
                            "Thread {}, recived a task of type: {}",
                            thread::current().name().unwrap(),
                            event.kind,
                        );
                        let res = (event.task)();
                        println!(
                            "Thread {}, finished running a task of type: {}.",
                            thread::current().name().unwrap(),
                            event.kind
                        );
                        threadp_sender.send((i, event.callback_id, res)).unwrap();
                    }
                })
                .expect("Couldn't initialize thread pool.");

            let node_thread = NodeThread {
                handle,
                sender: evt_sender,
            };

            threads.push(node_thread);
        }

        // ===== EPOLL THREAD =====
        // Only wakes up when there is a task ready
        let (epoll_sender, epoll_reciever) = channel::<usize>();
        let (epoll_start_sender, epoll_start_reciever) = channel::<()>();
        let queue = minimio::queue().expect("Error creating epoll queue");
        thread::spawn(move || {
            if 
              loop {
                match minimio::poll(queue, changes.as_mut_slice(), 0, None) {
                    Ok(v) if v > 0 => {
                        
                        // changes is reset and populated with new events
                        for i in 0..v {
                            let event = changes.get(i).expect("No events in event list.");
                            epoll_sender.send(event.ident as usize).unwrap();
                        }
                        
                    },
                    Err(e) => panic!("{:?}", e),
                    _ => (),
                }
                }
        });

        Runtime {
            thread_pool: threads.into_boxed_slice(),
            available: (0..4).collect(),
            callback_queue: Vec::new(),
            refs: 0,
            threadp_reciever,
            epoll_reciever,
            epoll_queue: queue,
            epoll_eventlist: vec![],
        }
    }

    // This is the event loop
    fn run(&mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = self;
        unsafe { RUNTIME = rt_ptr as usize };

        // First we run our "main" function
        f();
        while self.refs > 0 {
            // First poll any epoll/kqueue
            if let Ok(callback_id) = self.epoll_reciever.try_recv() {
                let cb = &self.callback_queue[callback_id];
                cb(Js::Undefined);
                self.refs -= 1;
            }

            // then check if there is any results from the threadpool
            if let Ok((thread_id, callback_id, data)) = self.threadp_reciever.try_recv() {
                let cb = &self.callback_queue[callback_id];
                cb(data);
                self.refs -= 1;
                self.available.push(thread_id);
            }
        }
    }

    fn schedule(&mut self) -> usize {
        match self.available.pop() {
            Some(thread_id) => thread_id,
            // We would normally queue this
            None => panic!("Out of threads."),
        }
    }

    fn register_io(&mut self, mut event: minimio::Event, cb: impl Fn(Js) + 'static) {
        self.callback_queue.push(Box::new(cb));

        let cb_id = self.callback_queue.len() - 1;
        // let's make this very simple and set the event identity equal to our callback id
        // this could be handled in many other (and better) ways though
        event.ident = cb_id as u64;
        minimio::add_event(self.epoll_queue, &[event.clone()], 0).expect("Error adding event to queue.");
        self.epoll_eventlist.push(event);
        println!("Event with id: {} registered.", cb_id);
        self.refs += 1;
    }

    fn register_work(
        &mut self,
        task: impl Fn() -> Js + Send + 'static,
        kind: EventKind,
        cb: impl Fn(Js) + 'static,
    ) {
        self.callback_queue.push(Box::new(cb));

        let event = Event {
            task: Box::new(task),
            callback_id: self.callback_queue.len() - 1,
            kind,
        };

        // we are not going to implement a real scheduler here, just a LIFO queue
        let available = self.schedule();
        self.thread_pool[available].sender.send(event).unwrap();
        self.refs += 1;
    }
}

// ===== THIS IS PLUGINS CREATED IN C++ FOR THE NODE RUNTIME OR PART OF THE RUNTIME ITSELF =====

struct Crypto;

impl Crypto {
    fn encrypt(n: usize, cb: impl Fn(Js) + 'static) {
        let work = move || {
            fn fibonacchi(n: usize) -> usize {
                match n {
                    0 => 0,
                    1 => 1,
                    _ => fibonacchi(n - 1) + fibonacchi(n - 2),
                }
            }

            let fib = fibonacchi(n);
            Js::Int(fib)
        };

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, EventKind::Encrypt, cb);
    }
}

struct Fs;
impl Fs {
    fn read(path: &'static str, cb: impl Fn(Js) + 'static) {
        let work = move || {
            // Let's simulate that there is a large file we're reading allowing us to actually
            // observe how the code is executed
            thread::sleep(std::time::Duration::from_secs(2));
            let mut buffer = String::new();
            fs::File::open(&path)
                .unwrap()
                .read_to_string(&mut buffer)
                .unwrap();
            Js::String(buffer)
        };

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, EventKind::FileRead, cb);
    }
}

// ===== THIS IS OUR EPOLL/KQUEUE/IOCP LIBRARY =====
use std::os::unix::io::RawFd;

struct Io;
impl Io {
    pub fn timeout(ms: u32, cb: impl Fn(Js) + 'static) {
        let event = minimio::event_timeout(ms as i64);

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };

        rt.register_io(event, cb);
    }
}

mod minimio {
    use super::*;
    pub fn queue() -> io::Result<i32> {
        if cfg!(target_os = "macos") {
            macos::kqueue()
        } else {
            unimplemented!()
        }
    }

    #[cfg(target_os = "macos")]
    pub type Event = macos::ffi::Kevent;

    pub fn poll(queue: i32, changelist: &mut [Event], timeout: usize, max_events: Option<i32>) -> io::Result<usize> {
        if cfg!(target_os = "macos") {
            macos::kevent(queue, &[], changelist, timeout)
        } else {
            unimplemented!()
        }
    }

    /// Timeout of 0 means no timeout
    pub fn add_event(queue: i32, event_list: &[Event], timeout_ms: usize) -> io::Result<usize> {
        if cfg!(target_os = "macos") {
            macos::kevent(queue, event_list, &mut [], timeout_ms)
        } else {
            unimplemented!()
        }
    }

    pub fn event_timeout(timeout_ms: i64) -> Event {
        if cfg!(target_os = "macos") {
            Event {
                ident: 0,
                filter: unsafe {macos::EVFILT_TIMER},
                flags: unsafe {macos::EV_ADD | macos::EV_ENABLE | macos::EV_ONESHOT},
                fflags: 0,
                data: timeout_ms,
                udata: 0,
                ext: [0,0],
            }
        } else {
            unimplemented!()
        }
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;
        use ffi::*;

        pub const EV_ADD: u16 = 0x1;
        pub const EVFILT_TIMER: i16 = -7;
        pub const EV_ENABLE: u16 = 0x4;
        pub const EV_ONESHOT: u16 = 0x10;

        pub mod ffi {
                #[derive(Debug, Clone)]
        #[repr(C)]
        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L497
        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L207
        pub struct Kevent {
            pub ident: u64,
            pub filter: i16,
            pub flags: u16,
            pub fflags: u32,
            pub data: i64,
            pub udata: u64,
            pub ext: [u64; 2],
        }
        #[link(name = "c")]
        extern {
            /// Returns: positive: file descriptor, negative: error
            pub(super) fn kqueue() -> i32;
            /// Returns: nothing, all non zero return values is an error
            //fn kevent(epfd: i32, op: i32, fd: i32, epoll_event: *const Kevent) -> i32;
            pub(super) fn kevent(
                kq: i32,
                changelist: *const Kevent,
                nchanges: i32,
                eventlist: *mut Kevent,
                nevents: i32,
                timeout: usize,
            ) -> i32;
        }
        }
        pub fn timeout_event(timer: i64) -> ffi::Kevent {
             let event = ffi::Kevent {
                ident: 1,
                filter: unsafe {EVFILT_TIMER},
                flags: unsafe {EV_ADD | EV_ENABLE | EV_ONESHOT},
                fflags: 0,
                data: timer,
                udata: 0,
                ext: [0,0],
            };
            event
        } 

        pub fn kqueue() -> io::Result<i32> {
            let fd = unsafe {ffi::kqueue()};
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(fd)
        }

        pub fn kevent(kq: RawFd, cl: &[Kevent], el: &mut [Kevent], timeout: usize) -> io::Result<usize> {
            let res = unsafe {
                let kq = kq as i32;
                let cl_len = cl.len() as i32;
                let el_len = el.len() as i32;
                ffi::kevent(kq, cl.as_ptr(), cl_len, el.as_mut_ptr(), el_len, timeout)
            };
            if res < 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(res as usize)
        }
    }
}