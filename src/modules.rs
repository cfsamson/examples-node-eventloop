// ===== THIS IS OUR EPOLL/KQUEUE/IOCP LIBRARY =====
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, RawFd};
use std::io::{self, Read, Write};
use std::fs;
use std::thread;

use crate::{Runtime, Js, EventKind};
use crate::minimio;
use crate::RUNTIME;

// ===== THIS IS PLUGINS CREATED IN C++ FOR THE NODE RUNTIME OR PART OF THE RUNTIME ITSELF =====
// The pointer dereferencing of our runtime is not striclty needed but is mostly for trying to
// emulate a bit of the same feeling as when you use modules in javascript. We could pass the runtime in
// as a reference to our startup function.

pub struct Crypto;
impl Crypto {
    pub fn encrypt(n: usize, cb: impl Fn(Js) + 'static + Clone) {
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
        rt.register_work(work, cb);
    }
}

pub struct Fs;
impl Fs {
    pub fn read(path: &'static str, cb: impl Fn(Js) + 'static) {
        let work = move || {
            // Let's simulate that there is a very large file we're reading allowing us to actually
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
        rt.register_work(work, cb);
    }
}


pub struct Io;
impl Io {
    pub fn http_get_slow(url: &str, delay_ms: u32, cb: impl Fn(Js) + 'static + Clone) {
        // Don't worry, http://slowwly.robertomurray.co.uk is a site for simulating a delayed
        // response from a server. Perfect for our use case.
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
            .expect("Error writing to stream");
        stream
            .set_nonblocking(true)
            .expect("set_nonblocking call failed");
        let fd = stream.as_raw_fd();

        let event = minimio::event_read(fd);

        let wrapped = move |_n| {
            let mut stream = stream;
            // we do this to prevent getting an error if the status somehow changes between the epoll
            // and our read. In a real implementation this should be handled by re-register the event
            // to the epoll queue for example or just accept that there might be a very small amount
            // of blocking happening here (it might even be more costly to re-register the task)
            stream
                .set_nonblocking(false)
                .expect("Error setting stream to blocking.");
            let mut buffer = String::new();
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            cb(Js::String(buffer));
        };

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, wrapped);
    }
    /// URl is in www.google.com format, i.e. only the host name, we can't
    /// request paths at this point
    pub fn http_get(url: &str, cb: impl Fn(Js) + 'static + Clone) {
        let url_port = format!("{}:80", url);
        let mut stream: TcpStream = TcpStream::connect(&url_port).unwrap();
        let request = format!(
            "GET / HTTP/1.1\r\n\
             Host: {}\r\n\
             Connection: close\r\n\
             \r\n",
            url
        );

        stream
            .write_all(request.as_bytes())
            .expect("Error writing to stream");
        stream
            .set_nonblocking(true)
            .expect("set_nonblocking call failed");
        let fd = stream.as_raw_fd();

        let event = minimio::event_read(fd);

        let wrapped = move |_n| {
            let mut stream = stream;
            let mut buffer = String::new();
            // we do this to prevent getting an error if the status somehow changes between the epoll
            // and our read. In a real implementation this should be handled by re-register the event
            // to the epoll queue for example or just accept that there might be a very small amount
            // of blocking happening here (it might even be more costly to re-register the task)
            stream
                .set_nonblocking(false)
                .expect("Error setting stream to blocking.");
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            // The way we do this we know it's a redirect so we grab the location header and
            // get that webpage instead
            cb(Js::String(buffer));
        };

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, wrapped);
    }
}
