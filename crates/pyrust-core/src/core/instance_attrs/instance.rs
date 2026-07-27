#[derive(Debug, Clone)]
pub struct PyInstance {
    pub class: Rc<RefCell<PyClass>>,
    pub attrs: InstanceAttrs,
}
