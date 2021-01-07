use crate::http::{Method, Request};
use crate::routing::{Filter, PathState};

pub struct MethodFilter(Method);

impl Filter for MethodFilter {
    #[inline]
    fn execute(&self, req: &mut Request, _path: &mut PathState) -> bool {
        req.method() == self.0
    }
}

pub fn get() -> MethodFilter {
    MethodFilter(Method::GET)
}
pub fn head() -> MethodFilter {
    MethodFilter(Method::HEAD)
}
pub fn options() -> MethodFilter {
    MethodFilter(Method::OPTIONS)
}
pub fn post() -> MethodFilter {
    MethodFilter(Method::POST)
}
pub fn patch() -> MethodFilter {
    MethodFilter(Method::PATCH)
}
pub fn put() -> MethodFilter {
    MethodFilter(Method::PUT)
}
pub fn delete() -> MethodFilter {
    MethodFilter(Method::DELETE)
}
