fn main() {
    let mut rt = Runtime::new();

    let main = || {
        println!("Thread: {}. I want to read test.txt", cur_thread());
        Fs::read("test.txt".to_string(), |result| {
            // this is easier when dealing with javascript since you cast it to the relevant type
            // and there are no more checks...
            let text = result.to_string().unwrap();
            println!(
                "Thread: {}. First count: {} characters.",
                cur_thread(),
                text.len()
            );

            println!("Thread: {}. I want to encrypt something.", cur_thread());
            Crypto::encrypt(text.len(), |result| {
                let n = result.to_int().unwrap();
                println!(
                    "Thread: {}. The \"encrypted\" number is: {}",
                    cur_thread(),
                    n
                );
            })
        });

        // let's read the fil again and display the text
        println!(
            "Thread: {}. I want to read test.txt a second time",
            cur_thread()
        );
        Fs::read("test.txt".to_string(), |result| {
            let text = result.to_string().unwrap();
            println!(
                "Thread: {}. Second count: {} characters.",
                cur_thread(),
                text.len()
            );

            // aaand one more time once the second time has finished...
            println!(
                "Thread: {}. I want to read test.txt a third time and then print the text",
                thread::current().name().unwrap()
            );
            Fs::read("test.txt".to_string(), |result| {
                let text = result.to_string().unwrap();
                println!(
                    "Thread: {}. The file contains the following text:\n\n\"{}\"\n",
                    cur_thread(),
                    text
                );
            });
        });
    };

    rt.run(main);
}

fn cur_thread() -> String {
    thread::current().name().unwrap().to_string()
}

static mut RUNTIME: usize = 0;

use std::fmt;
use std::fs;
use std::io::Read;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};

struct Runtime {
    thread_pool: Box<[NodeThread]>,
    available: Vec<usize>,
    callback_queue: Vec<Callback>,
    refs: usize,
    status_reciever: Receiver<(usize, usize, Js)>,
}

impl Runtime {
    fn new() -> Self {
        let (status_sender, status_reciever) = channel::<(usize, usize, Js)>();
        let mut threads = Vec::with_capacity(4);

        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let status_sender = status_sender.clone();
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
                        status_sender.send((i, event.callback_id, res)).unwrap();
                    }
                })
                .expect("Couldn't initialize thread pool.");

            let node_thread = NodeThread {
                handle,
                sender: evt_sender,
            };

            threads.push(node_thread);
        }

        Runtime {
            thread_pool: threads.into_boxed_slice(),
            available: (0..4).collect(),
            callback_queue: Vec::new(),
            refs: 0,
            status_reciever,
        }
    }

    fn run(&mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = self;
        unsafe { RUNTIME = rt_ptr as usize };

        f();
        while self.refs > 0 {
            // First poll any epoll/kqueue

            // then check if there is any results from the threadpool
            if let Ok((thread_id, callback_id, data)) = self.status_reciever.try_recv() {
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
struct NodeThread {
    handle: JoinHandle<()>,
    sender: Sender<Event>,
}

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
    fn read(path: String, cb: impl Fn(Js) + 'static) {
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

struct Event {
    task: Box<Fn() -> Js + Send + 'static>,
    callback_id: usize,
    kind: EventKind,
}

type Callback = Box<Fn(Js)>;
