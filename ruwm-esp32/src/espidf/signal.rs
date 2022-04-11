extern crate alloc;
use alloc::sync::Arc;

use embedded_svc::channel::asyncs::*;
use embedded_svc::utils::asyncs::signal;
use embedded_svc::utils::asyncs::signal::adapt::SignalReceiver;
use embedded_svc::utils::asyncs::signal::adapt::SignalSender;

use esp_idf_hal::mutex::Mutex;

use ruwm::broadcast_binder;
use ruwm::error;

pub struct SignalFactory;

impl<'a> broadcast_binder::SignalFactory<'a> for SignalFactory {
    type Sender<D>
    where
        D: 'a,
    = impl Sender<Data = D>;

    type Receiver<D>
    where
        D: 'a,
    = impl Receiver<Data = D>;

    fn create<D>(&mut self) -> error::Result<(Self::Sender<D>, Self::Receiver<D>)>
    where
        D: Send + Sync + Clone + 'a,
    {
        signal()
    }
}

pub fn signal<'a, T>() -> error::Result<(
    impl Sender<Data = T> + Clone,
    impl Receiver<Data = T> + Clone,
)>
where
    T: Send + Sync + Clone + 'a,
{
    let signal = Arc::new(signal::MutexSignal::<Mutex<signal::State<T>>, T>::new());

    Ok((
        SignalSender::new(signal.clone()),
        SignalReceiver::new(signal),
    ))
}
