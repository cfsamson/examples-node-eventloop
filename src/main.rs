fn main() {
    let mut rt = Runtime::new();

    let main = || {
        println!(
            "Thread: {}. I want to read a file",
            thread::current().name().unwrap()
        );
        Fs::read("test.txt".to_string(), |result| {
            // we could use a pointer and just cast it to our desired type
            let text = if let Ops::FileRead(text) = result {
                println!(
                    "Thread: {}. The file contains {} characters.",
                    thread::current().name().unwrap(),
                    text.len()
                );
                text
            } else {
                panic!("Invalid result type.")
            };

            println!(
                "Thread: {}. I want to encrypt something.",
                thread::current().name().unwrap()
            );
            Crypto::encrypt(text.len(), |result| {
                if let Ops::Encrypt(n) = result {
                    println!(
                        "Thread: {}. The \"encrypted\" number is: {}",
                        thread::current().name().unwrap(),
                        n
                    );
                }
            })
        });

        // let's read the fil again and display the text
        println!(
            "Thread: {}. I want to read the file a second time",
            thread::current().name().unwrap()
        );
        Fs::read("test.txt".to_string(), |result| {
            if let Ops::FileRead(text) = result {
                println!(
                    "Thread: {}. The file contains the following text:\n\n\"{}\"\n",
                    thread::current().name().unwrap(),
                    text
                );
            };
        });

         // aaand one more time...
        println!(
            "Thread: {}. I want to read the file a third time",
            thread::current().name().unwrap()
        );
        Fs::read("test.txt".to_string(), |result| {
            if let Ops::FileRead(text) = result {
                println!(
                    "Thread: {}. The file contains the following text:\n\n\"{}\"\n",
                    thread::current().name().unwrap(),
                    text
                );
            };
        });
    };

   


    rt.run(main);
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
    status_reciever: Receiver<(usize, usize, Ops)>,
}

impl Runtime {
    fn new() -> Self {
        let (status_sender, status_reciever) = channel::<(usize, usize, Ops)>();
        let mut threads = Vec::with_capacity(4);

        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let status_sender = status_sender.clone();
            let handle = thread::Builder::new()
                .name(i.to_string())
                .spawn(move || {
                    while let Ok(event) = evt_reciever.recv() {
                        println!(
                            "Thread {}, recived a task.",
                            thread::current().name().unwrap()
                        );
                        let res = (event.task)();
                        println!(
                            "Thread {}, finished running a task of type: {}.",
                            thread::current().name().unwrap(),
                            res
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
        task: impl Fn() -> Ops + Send + 'static,
        cb: impl Fn(Ops) + 'static,
    ) {
        self.callback_queue.push(Box::new(cb));

        let event = Event {
            task: Box::new(task),
            callback_id: self.callback_queue.len() - 1,
        };

        // we are not going to implement a real scheduler here, just a LIFO queue
        let available = self.schedule();
        self.thread_pool[available].sender.send(event).unwrap();
        self.refs += 1;
    }
}

#[derive(Debug)]
enum Ops {
    Nothing,
    FileRead(String),
    Encrypt(usize),
}

impl fmt::Display for Ops {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Ops::*;
        match self {
            Nothing => write!(f, "Nothing"),
            FileRead(_) => write!(f, "File read"),
            Encrypt(_) => write!(f, "Encrypt"),
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
    fn encrypt(n: usize, cb: impl Fn(Ops) + 'static) {
        let work = move || {
            fn fibonacchi(n: usize) -> usize {
                match n {
                    0 => 0,
                    1 => 1,
                    _ => fibonacchi(n - 1) + fibonacchi(n - 2),
                }
            }

            let fib = fibonacchi(n);
            Ops::Encrypt(fib)
        };

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, cb);
    }
}

struct Fs;
impl Fs {
    fn read(path: String, cb: impl Fn(Ops) + 'static) {
        let work = move || {
            // Let's simulate that there is a large file we're reading allowing us to actually
            // observe how the code is executed
            thread::sleep(std::time::Duration::from_secs(2));
            let mut buffer = String::new();
            fs::File::open(&path)
                .unwrap()
                .read_to_string(&mut buffer)
                .unwrap();
            Ops::FileRead(buffer)
        };

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, cb);
    }
}

struct Event {
    task: Box<Fn() -> Ops + Send + 'static>,
    callback_id: usize,
}

type Callback = Box<Fn(Ops)>;
