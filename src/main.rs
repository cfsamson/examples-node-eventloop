/// Think of this function as the javascript program you have written
fn javascript() {
    p("First call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        p(format!("First count: {} characters.", len));

        p("I want to encrypt something.");
        Crypto::encrypt(text.len(), |result| {
            let n = result.into_int().unwrap();
            p(format!("\"Encrypted\" number is: {}", n));
        })
    });

    p("Registering immediate timeout 1");
    Io::timeout(0, |_res| {
        p("Immediate1 timed out");
    });
    p("Registering immediate timeout 2");
    Io::timeout(0, |_res| {
        p("Immediate2 timed out");
    });
    p("Registering immediate timeout 3");
    Io::timeout(0, |_res| {
        p("Immediate3 timed out");
    });

    p("Second call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        p(format!("Second count: {} characters.", len));

        p("Third call to read test.txt");
        Fs::read("test.txt", |result| {
            let text = result.into_string().unwrap();
            p_content(&text, "file read");
        });
    });

    p("Registering a 3000 ms timeout");
    Io::timeout(1000, |_res| {
        p("3000ms timer timed out");
        Io::timeout(500, |_res| {
            p("1500ms timer(nested) timed out");
        });
    });

    p("Registering http get request to google.com");
    Io::http_get_slow("http//www.google.com", 2000, |result| {
        let result = result.into_string().unwrap();
        p_content(result.trim(), "web call");
    });
}
fn p(t: impl std::fmt::Display) {
    println!("Thread: {}\t {}", current(), t);
}

fn p_content(t: impl std::fmt::Display, decr: &str) {
    println!(
        "\n===== THREAD {} START CONTENT - {} =====",
        current(),
        decr.to_uppercase()
    );
    println!("{}", t);
    println!("===== END CONTENT =====\n");
}

fn current() -> String {
    thread::current().name().unwrap().to_string()
}

fn main() {
    let mut rt = Runtime::new();
    rt.run(javascript);
}

// ===== THIS IS OUR "NODE LIBRARY" =====
use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};

mod minimio;

static mut RUNTIME: usize = 0;

type Callback = Box<FnOnce(Js)>;

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
    fn into_string(self) -> Option<String> {
        match self {
            Js::String(s) => Some(s),
            _ => None,
        }
    }

    fn into_int(self) -> Option<usize> {
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
    callback_queue: HashMap<usize, Callback>,
    identity_token: usize,
    refs: usize,
    threadp_reciever: Receiver<(usize, usize, Js)>,
    epoll_reciever: Receiver<usize>,
    epoll_queue: i32,
    epoll_pending: usize,
    epoll_starter: Sender<usize>,
    epoll_event_cb_map: HashMap<i64, usize>,
}

impl Runtime {
    fn new() -> Self {
        // ===== THE REGULAR THREADPOOL =====
        let (threadp_sender, threadp_reciever) = channel::<(usize, usize, Js)>();
        let mut threads = Vec::with_capacity(4);
        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let threadp_sender = threadp_sender.clone();
            let handle = thread::Builder::new()
                .name(format!("pool{}", i))
                .spawn(move || {
                    while let Ok(event) = evt_reciever.recv() {
                        p(format!("recived a task of type: {}", event.kind));
                        let res = (event.task)();
                        p(format!("finished running a task of type: {}.", event.kind));
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
        let (epoll_sender, epoll_reciever) = channel::<usize>();
        let (epoll_start_sender, epoll_start_reciever) = channel::<usize>();
        let queue = minimio::queue().expect("Error creating epoll queue");
        thread::Builder::new()
            .name("epoll".to_string())
            .spawn(move || loop {
                let mut changes = vec![];
                while let Ok(current_event_count) = epoll_start_reciever.recv() {
                    if changes.len() < current_event_count {
                        let missing = current_event_count - changes.len();
                        (0..missing).for_each(|_| changes.push(minimio::Event::default()));
                    }
                    match minimio::poll(queue, changes.as_mut_slice(), 0, None) {
                        Ok(v) if v > 0 => {
                            for i in 0..v {
                                let event = changes.get_mut(i).expect("No events in event list.");
                                p(format!("epoll event {} is ready", event.ident));
                                epoll_sender.send(event.ident as usize).unwrap();
                            }
                        }
                        Err(e) => panic!("{:?}", e),
                        _ => (),
                    }
                }
            })
            .expect("Error creating epoll thread");

        Runtime {
            thread_pool: threads.into_boxed_slice(),
            available: (0..4).collect(),
            callback_queue: HashMap::new(),
            identity_token: 0,
            refs: 0,
            threadp_reciever,
            epoll_reciever,
            epoll_queue: queue,
            epoll_pending: 0,
            epoll_starter: epoll_start_sender,
            epoll_event_cb_map: HashMap::new(),
        }
    }

    fn run(&mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = self;
        unsafe { RUNTIME = rt_ptr as usize };

        f();

        while self.refs > 0 {
            if let Ok(event_id) = self.epoll_reciever.try_recv() {
                let id = self
                    .epoll_event_cb_map
                    .get(&(event_id as i64))
                    .expect("Event not in event map.");
                let callback_id = *id;
                self.epoll_event_cb_map.remove(&(event_id as i64));

                let cb = self.callback_queue.remove(&callback_id).unwrap();
                cb(Js::Undefined);
                self.refs -= 1;
                self.epoll_pending -= 1;
            }

            if let Ok((thread_id, callback_id, data)) = self.threadp_reciever.try_recv() {
                let cb = self.callback_queue.remove(&callback_id).unwrap();
                cb(data);
                self.refs -= 1;
                self.available.push(thread_id);
            }

            thread::sleep(std::time::Duration::from_millis(1));
        }
        p("FINISHED");
    }

    fn schedule(&mut self) -> usize {
        match self.available.pop() {
            Some(thread_id) => thread_id,
            None => panic!("Out of threads."),
        }
    }

    fn generate_identity(&mut self) -> usize {
        self.identity_token = self.identity_token.wrapping_add(1);
        self.identity_token
    }

    fn add_callback<CB>(&mut self, cb: CB) -> usize
    where
        CB: FnOnce(Js) + 'static,
    {
        let ident = self.generate_identity();
        let boxed_cb = Box::new(cb);
        let taken = self.callback_queue.contains_key(&ident);

        if !taken {
            self.callback_queue.insert(ident, boxed_cb);
            ident
        } else {
            loop {
                let possible_ident = self.generate_identity();
                if self.callback_queue.contains_key(&possible_ident) {
                    self.callback_queue.insert(possible_ident, boxed_cb);
                    break possible_ident;
                }
            }
        }
    }

    fn register_io<CB>(&mut self, mut event: minimio::Event, cb: CB)
    where
        CB: FnOnce(Js) + 'static,
    {
        let cb_id = self.add_callback(cb) as i64;

        if event.ident == 0 {
            event.ident = cb_id as u64 + 1_000_000;
        }
        p(format!("Event with id: {} registered.", event.ident));
        self.epoll_event_cb_map
            .insert(event.ident as i64, cb_id as usize);

        minimio::add_event(self.epoll_queue, &[event.clone()], 0)
            .expect("Error adding event to queue.");

        self.refs += 1;
        self.epoll_pending += 1;
        self.epoll_starter
            .send(self.epoll_pending)
            .expect("Sending to epoll_starter.");
    }

    fn register_work<T, CB>(&mut self, task: T, kind: EventKind, cb: CB)
    where
        T: Fn() -> Js + Send + 'static,
        CB: FnOnce(Js) + 'static,
    {
        let callback_id = self.add_callback(cb);
        let event = Event {
            task: Box::new(task),
            callback_id,
            kind,
        };

        let available = self.schedule();
        self.thread_pool[available].sender.send(event).unwrap();
        self.refs += 1;
    }
}

// ===== THIS IS PLUGINS CREATED IN C++ FOR THE NODE RUNTIME OR PART OF THE RUNTIME ITSELF =====
use std::fs::File;
struct Crypto;

impl Crypto {
    fn encrypt<CB>(n: usize, cb: CB)
    where
        CB: Fn(Js) + 'static + Clone,
    {
        let work = move || {
            fn fibonacchi(n: usize) -> usize {
                match n {
                    0 => 0,
                    1 => 1,
                    _ => fibonacchi(n - 1) + fibonacchi(n - 2),
                }
            }
            Js::Int(fibonacchi(n))
        };

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, EventKind::Encrypt, cb);
    }
}

struct Fs;
impl Fs {
    fn read<CB>(path: &'static str, cb: CB)
    where
        CB: Fn(Js) + 'static,
    {
        let work = move || {
            thread::sleep(std::time::Duration::from_secs(2));
            let mut buffer = String::new();
            File::open(&path)
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
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, RawFd};

struct Io;
impl Io {
    pub fn timeout(ms: u32, cb: impl Fn(Js) + 'static) {
        let event = minimio::event_timeout(i64::from(ms));
        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, cb);
    }

    pub fn http_get_slow<CB>(url: &str, delay_ms: u32, cb: CB)
    where
        CB: Fn(Js) + 'static + Clone,
    {
        let mut stream: TcpStream = TcpStream::connect("slowwly.robertomurray.co.uk:80").unwrap();
        let request = format!(
            "GET /delay/{}/url/http://{} HTTP/1.1\r\n\
             Host: slowwly.robertomurray.co.uk\r\n\
             Connection: close\r\n\
             \r\n",
            delay_ms, url
        );

        stream
            .write_all(request.as_bytes())
            .expect("Writing to stream");
        stream
            .set_nonblocking(true)
            .expect("set_nonblocking failed");

        let fd = stream.as_raw_fd();
        let event = minimio::event_read(fd);

        let wrapped = move |_n| {
            let mut stream = stream; 
            let mut buffer = String::new();
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            cb(Js::String(buffer));
        };

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, wrapped);
    }
}