trait Handler {
  fn on_message(&mut self, token: u16, data: &[u8]);
  fn on_stream_data(&mut self, token: u16, data: &[u8]);
  fn on_disconnect(&mut self);
}

struct Client;
impl Client {
  pub fn poll<H: Handler>(&mut self, handler: &mut H) {
    todo!()
  }
}

struct State;
impl State {
  fn update(&mut self) {
    todo!()
  }
}
impl Handler for State {
  fn on_message(&mut self, token: u16, data: &[u8]) {
    todo!()
  }

  fn on_stream_data(&mut self, token: u16, data: &[u8]) {
    todo!()
  }

  fn on_disconnect(&mut self) {
    todo!()
  }
}

fn main() {
  let mut client = Client;
  let mut state = State;

  client.poll(&mut state);
  state.update();
}
