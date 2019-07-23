use js_event_loop::{Runtime, set_timeout};
use js_event_loop::modules::{Fs, Crypto, Io};
use std::thread;
/// Think of this function as the javascript program you have written
fn javascript() {
    print("First call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        print(format!("First count: {} characters.", len));

        print("I want to encrypt something.");
        Crypto::encrypt(text.len(), |result| {
            let n = result.into_int().unwrap();
            print(format!("\"Encrypted\" number is: {}", n));
        })
    });

    print("Registering immediate timeout 1");
    set_timeout(0, |_res| {
        print("Immediate1 timed out");
    });
    print("Registering immediate timeout 2");
    set_timeout(0, |_res| {
        print("Immediate2 timed out");
    });
    print("Registering immediate timeout 3");
    set_timeout(0, |_res| {
        print("Immediate3 timed out");
    });

    // let's read the file again and display the text
    print("Second call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        print(format!("Second count: {} characters.", len));

        // aaand one more time but not in parallell.
        print("Third call to read test.txt");
        Fs::read("test.txt", |result| {
            let text = result.into_string().unwrap();
            print_content(&text, "file read");
        });
    });

    print("Registering a 3000 ms timeout");
    set_timeout(3000, |_res| {
        print("3000ms timer timed out");
        set_timeout(500, |_res| {
            print("500ms timer(nested) timed out");
        });
    });

    print("Registering a 1000 ms timeout");
    set_timeout(1000, |_res| {
        print("SETTIMEOUT");
    });

    print("Registering http get request to google.com");
    Io::http_get_slow("http//www.google.com", 2000, |result| {
        let result = result.into_string().unwrap();
        print_content(result.trim(), "web call");
    });
}
fn print(t: impl std::fmt::Display) {
    println!("Thread: {}\t {}", current(), t);
}

fn print_content(t: impl std::fmt::Display, decr: &str) {
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

#[test]
fn main() {
    let mut rt = Runtime::new();
    rt.run(javascript);
}