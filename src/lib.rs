#![cfg_attr(not(feature = "std"), no_std)]

use core::marker::PhantomData;

use minicoro_sys::*;

pub trait Allocator {
    unsafe fn allocate(&self, size: usize) -> *mut u8;
    unsafe fn deallocate(&self, ptr: *mut u8);
}

#[cfg(feature = "std")]
pub struct GLOBAL;

#[cfg(feature = "std")]
extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
}

#[cfg(feature = "std")]
impl Allocator for GLOBAL {
    unsafe fn allocate(&self, size: usize) -> *mut u8 {
        malloc(size)
    }

    unsafe fn deallocate(&self, ptr: *mut u8) {
        free(ptr)
    }
}

#[cfg(feature = "std")]
#[repr(C)]
pub struct Coroutine<VALUE, YIELD, RET, DATA, A: Allocator = GLOBAL> {
    co: *const mco_coro,
    _d: PhantomData<(VALUE, YIELD, RET, DATA, A)>,
}

#[cfg(not(feature = "std"))]
#[repr(C)]
pub struct Coroutine<VALUE, YIELD, RET, DATA, A: Allocator> {
    co: *const mco_coro,
    _d: PhantomData<(VALUE, YIELD, RET, DATA, A)>,
}

pub enum CoroutineResult<YIELD, RET> {
    Yield(YIELD),
    Return(RET),
    Error(&'static str),
}

struct UserData<VALUE, YIELD, RET, DATA, A: Allocator> {
    routine: *mut dyn Fn(CoroutineRef<VALUE, YIELD, RET, DATA, A>) -> RET,
    allocator: A,
    data: DATA,
}

pub struct CoroutineRef<VALUE, YIELD, RET, DATA, A: Allocator> {
    co: *const mco_coro,
    _d: PhantomData<(VALUE, YIELD, RET, DATA, A)>,
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> Clone for CoroutineRef<VALUE, YIELD, RET, DATA, A> {
    fn clone(&self) -> Self {
        Self {
            co: self.co,
            _d: PhantomData,
        }
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> Copy for CoroutineRef<VALUE, YIELD, RET, DATA, A> {}

unsafe extern "C" fn co_malloc<VALUE, YIELD, RET, DATA, A: Allocator>(
    size: usize,
    data: *const (),
) -> *const () {
    let data = (data as *const UserData<VALUE, YIELD, RET, DATA, A>)
        .as_ref()
        .unwrap();
    let ptr = data.allocator.allocate(size);

    return ptr as _;
}

unsafe extern "C" fn co_free<VALUE, YIELD, RET, DATA, A: Allocator>(
    ptr: *const (),
    data: *const (),
) {
    let data = (data as *const UserData<VALUE, YIELD, RET, DATA, A>)
        .as_ref()
        .unwrap();
    data.allocator.deallocate(ptr as _);
}

extern "C" fn coroutine_wrapper<VALUE, YIELD, RET, DATA, A: Allocator>(co: *const mco_coro) {
    unsafe {
        let data = (mco_get_user_data(co) as *const UserData<VALUE, YIELD, RET, DATA, A>)
            .as_ref()
            .unwrap();

        let r = CoroutineRef {
            co: co,
            _d: PhantomData,
        };
        let result = (data.routine.as_ref().unwrap())(r);

        r.return_(result);
    }
}

#[cfg(feature = "std")]
impl<VALUE, YIELD, RET, DATA> Coroutine<VALUE, YIELD, RET, DATA> {
    pub fn new<F>(routine_function: F, user_data: DATA) -> Result<Self, &'static str>
    where
        F: Fn(CoroutineRef<VALUE, YIELD, RET, DATA, GLOBAL>) -> RET + 'static,
    {
        Self::new_in(routine_function, user_data, GLOBAL)
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> Coroutine<VALUE, YIELD, RET, DATA, A> {
    pub fn new_in<F>(
        routine_function: F,
        user_data: DATA,
        allocator: A,
    ) -> Result<Self, &'static str>
    where
        F: Fn(CoroutineRef<VALUE, YIELD, RET, DATA, A>) -> RET + 'static,
    {
        unsafe {
            let func = allocator.allocate(core::mem::size_of::<F>()) as *mut F;
            func.write(routine_function);

            let mut desc = mco_desc_init(coroutine_wrapper::<VALUE, YIELD, RET, DATA, A>, 0);

            let ud = allocator
                .allocate(core::mem::size_of::<UserData<VALUE, YIELD, RET, DATA, A>>())
                as *mut UserData<VALUE, YIELD, RET, DATA, A>;

            ud.write(UserData {
                routine: func as *mut dyn Fn(CoroutineRef<VALUE, YIELD, RET, DATA, A>) -> RET,
                allocator: allocator,
                data: user_data,
            });

            desc.user_data = ud as _;
            desc.malloc_cb = co_malloc::<VALUE, YIELD, RET, DATA, A>;
            desc.free_cb = co_free::<VALUE, YIELD, RET, DATA, A>;

            let mut coro: *const mco_coro = 0 as _;

            let re = mco_create(&mut coro, &desc);

            if re != mco_result::MCO_SUCCESS {
                return Err(error_to_str(re));
            }

            Ok(Self {
                co: coro,
                _d: Default::default(),
            })
        }
    }

    #[inline]
    pub fn running() -> Option<CoroutineRef<VALUE, YIELD, RET, DATA, A>> {
        let p = unsafe { mco_running() };
        if p.is_null() {
            None
        } else {
            Some(CoroutineRef {
                co: p,
                _d: PhantomData,
            })
        }
    }

    #[inline]
    pub fn user_data<'a>(&'a self) -> &'a DATA {
        unsafe {
            let d = (mco_get_user_data(self.co) as *const UserData<VALUE, YIELD, RET, DATA, A>)
                .as_ref()
                .expect("");
            return &d.data;
        }
    }

    /// return none if coroutine returned
    #[inline]
    pub fn resume(&self, value: VALUE) -> Option<CoroutineResult<YIELD, RET>> {
        unsafe {
            let current_status = mco_status(self.co);
            if current_status == mco_state::MCO_DEAD {
                return None;
            }

            if current_status == mco_state::MCO_RUNNING {
                return Some(CoroutineResult::Error("resume on runnig coroutine"));
            }

            let re = mco_push(
                self.co,
                &value as *const VALUE as _,
                core::mem::size_of::<VALUE>(),
            );

            if re != mco_result::MCO_SUCCESS {
                return Some(CoroutineResult::Error(error_to_str(re)));
            }

            let re = mco_resume(self.co);

            if re != mco_result::MCO_SUCCESS {
                return Some(CoroutineResult::Error(error_to_str(re)));
            }

            if mco_status(self.co) == mco_state::MCO_DEAD {
                let mut value: RET = core::mem::zeroed();

                let re = mco_pop(
                    self.co,
                    &mut value as *mut RET as *const u8,
                    core::mem::size_of::<RET>(),
                );

                if re != mco_result::MCO_SUCCESS {
                    return Some(CoroutineResult::Error(error_to_str(re)));
                }

                return Some(CoroutineResult::Return(value));
            } else {
                let mut value: YIELD = core::mem::zeroed();

                let re = mco_pop(
                    self.co,
                    &mut value as *mut YIELD as *const u8,
                    core::mem::size_of::<RET>(),
                );

                if re != mco_result::MCO_SUCCESS {
                    return Some(CoroutineResult::Error(error_to_str(re)));
                }

                return Some(CoroutineResult::Yield(value));
            }
        }
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> Drop for Coroutine<VALUE, YIELD, RET, DATA, A> {
    fn drop(&mut self) {
        unsafe {
            core::ptr::drop_in_place(self.user_data() as *const DATA as *mut DATA);
            mco_destroy(self.co);
        }
    }
}

impl<VALUE: Default, YIELD, RET, DATA> core::future::Future for Coroutine<VALUE, YIELD, RET, DATA> {
    type Output = Result<RET, &'static str>;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        match self.resume(Default::default()) {
            Some(r) => match r {
                CoroutineResult::Return(r) => core::task::Poll::Ready(Ok(r)),
                CoroutineResult::Error(e) => core::task::Poll::Ready(Err(e)),
                _ => core::task::Poll::Pending,
            },
            None => return core::task::Poll::Pending,
        }
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> CoroutineRef<VALUE, YIELD, RET, DATA, A> {
    #[inline]
    pub fn yield_(&self, yield_value: YIELD) -> VALUE {
        unsafe {
            let value: VALUE = core::mem::zeroed();
            mco_pop(
                self.co,
                &value as *const VALUE as _,
                core::mem::size_of::<VALUE>(),
            );

            mco_push(
                self.co,
                &yield_value as *const YIELD as _,
                core::mem::size_of::<YIELD>(),
            );

            mco_yield(self.co);

            core::mem::forget(yield_value);

            return value;
        }
    }

    #[inline]
    pub fn return_(&self, value: RET) {
        unsafe {
            let v: VALUE = core::mem::zeroed();
            mco_pop(
                self.co,
                &v as *const VALUE as _,
                core::mem::size_of::<VALUE>(),
            );

            mco_push(
                self.co,
                &value as *const RET as _,
                core::mem::size_of::<RET>(),
            );

            core::mem::forget(value);
        }
    }

    #[inline]
    pub fn user_data<'a>(&'a self) -> &'a DATA {
        unsafe {
            let d = (mco_get_user_data(self.co) as *const UserData<VALUE, YIELD, RET, DATA, A>)
                .as_ref()
                .expect("");
            return &d.data;
        }
    }
}

fn error_to_str(e: mco_result) -> &'static str {
    match e {
        mco_result::MCO_SUCCESS => "No error",
        mco_result::MCO_GENERIC_ERROR => "Generic error",
        mco_result::MCO_INVALID_ARGUMENTS => "Invalid arguments",
        mco_result::MCO_INVALID_COROUTINE => "Invalid coroutine",
        mco_result::MCO_INVALID_OPERATION => "Invalid operation",
        mco_result::MCO_INVALID_POINTER => "Invalid pointer",
        mco_result::MCO_MAKE_CONTEXT_ERROR => "Make context error",
        mco_result::MCO_NOT_ENOUGH_SPACE => "Not enough space",
        mco_result::MCO_NOT_RUNNING => "Not running",
        mco_result::MCO_NOT_SUSPENDED => "Not suspended",
        mco_result::MCO_OUT_OF_MEMORY => "Out of Memory",
        mco_result::MCO_STACK_OVERFLOW => "Stack overflow",
        mco_result::MCO_SWITCH_CONTEXT_ERROR => "Switch context error",
    }
}
