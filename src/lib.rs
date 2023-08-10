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

// coroutine can be send to another thread
unsafe impl<VALUE, YIELD, RET, DATA, A:Allocator> Send for Coroutine<VALUE, YIELD, RET, DATA, A>{}

pub enum CoroutineResult<YIELD, RET> {
    Yield(YIELD),
    Return(RET),
    Error(&'static str),
}

struct UserData<VALUE, YIELD, RET, DATA, A: Allocator> {
    routine: *mut dyn Fn(CoroutineRef<VALUE, YIELD, RET, DATA, A>) -> RET,
    allocator: A,
    values: Option<VALUE>,
    yields: Option<YIELD>,
    returns: Option<RET>,
    returned: bool,
    data: DATA,
}


pub struct CoroutineRef<VALUE, YIELD, RET, DATA, A: Allocator> {
    // corotine ref cannot be send
    co: *mut mco_coro,
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
            co: co as *mut mco_coro,
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
                yields: None,
                values: None,
                returns: None,
                returned: false,
                data: user_data,
            });

            desc.user_data = ud as _;
            desc.allocator_data = ud as _;
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
                co: p as *mut mco_coro,
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
    pub fn resume(&mut self, value: VALUE) -> Option<CoroutineResult<YIELD, RET>> {
        unsafe {
            let current_status = mco_status(self.co);
            if current_status == mco_state::MCO_DEAD {
                return None;
            }

            if current_status == mco_state::MCO_RUNNING {
                return Some(CoroutineResult::Error("resume on runnig coroutine"));
            }

            let data = (mco_get_user_data(self.co) as *mut UserData<VALUE, YIELD, RET, DATA, A>)
                .as_mut()
                .unwrap_unchecked();

            if data.returned{
                return None;
            }

            data.values = Some(value);

            let re = mco_resume(self.co);

            if re != mco_result::MCO_SUCCESS {
                return Some(CoroutineResult::Error(error_to_str(re)));
            }

            if data.yields.is_none() {
                let mut value = None;
                core::mem::swap(&mut value, &mut data.returns);

                data.returned = true;

                return Some(CoroutineResult::Return(value.unwrap()));

            } else {
                let mut value = None;

                core::mem::swap(&mut value, &mut data.yields);

                return Some(CoroutineResult::Yield(value.unwrap()));
            }
        }
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> Drop for Coroutine<VALUE, YIELD, RET, DATA, A> {
    fn drop(&mut self) {
        unsafe {
            if !self.co.is_null(){
                core::ptr::drop_in_place(mco_get_user_data(self.co) as *mut UserData<VALUE, YIELD, RET, DATA, A>);
                mco_destroy(self.co);
            }
            
        }
    }
}

impl<VALUE, YIELD, RET, DATA, A: Allocator> CoroutineRef<VALUE, YIELD, RET, DATA, A> {
    #[inline]
    pub fn yield_(&self, yield_value: YIELD) -> VALUE {
        unsafe {
            // read from values
            let data = (mco_get_user_data(self.co) as *mut UserData<VALUE, YIELD, RET, DATA, A>)
                .as_mut()
                .unwrap_unchecked();

            let mut value = None;
            core::mem::swap(&mut value, &mut data.values);
            let value = value.unwrap_unchecked();

            data.yields = Some(yield_value);

            mco_yield(self.co);

            return value;
        }
    }

    #[inline]
    fn return_(&self, ret: RET) {
        unsafe {
            // read from values
            let data = (mco_get_user_data(self.co) as *mut UserData<VALUE, YIELD, RET, DATA, A>)
                .as_mut()
                .unwrap_unchecked();

            let mut value = None;
            core::mem::swap(&mut value, &mut data.values);
            let _value = value.unwrap_unchecked();

            data.returns = Some(ret);

            mco_yield(self.co);
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

impl<YIELD: Clone, RET: Clone> Clone for CoroutineResult<YIELD, RET> {
    fn clone(&self) -> Self {
        match self {
            CoroutineResult::Error(e) => CoroutineResult::Error(e),
            CoroutineResult::Return(r) => CoroutineResult::Return(r.clone()),
            CoroutineResult::Yield(y) => CoroutineResult::Yield(y.clone()),
        }
    }
}

impl<YIELD: Copy, RET:Copy> Copy for CoroutineResult<YIELD, RET>{}

impl<YIELD:core::hash::Hash, RET:core::hash::Hash> core::hash::Hash for CoroutineResult<YIELD, RET>{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        match self{
            Self::Error(e) => {
                state.write_u8(0);
                e.hash(state);
            },
            Self::Return(r) => {
                state.write_u8(1);
                r.hash(state);
            },
            Self::Yield(y) => {
                state.write_u8(2);
                y.hash(state);
            }
        }
    }
}

impl<YIELD: PartialEq, RET: PartialEq> PartialEq for CoroutineResult<YIELD, RET> {
    fn eq(&self, other: &Self) -> bool {
        match self {
            CoroutineResult::Error(e) => {
                if let CoroutineResult::Error(b) = other {
                    return e.eq(b);
                }
            }
            CoroutineResult::Return(r) => {
                if let CoroutineResult::Return(b) = other {
                    return r.eq(b);
                }
            }
            CoroutineResult::Yield(y) => {
                if let CoroutineResult::Yield(b) = other {
                    return y.eq(b);
                }
            }
        };

        return false;
    }
}

impl<YIELD:Eq, RET:Eq> Eq for CoroutineResult<YIELD, RET>{}

#[cfg(feature = "std")]
impl<YIELD: core::fmt::Debug, RET: core::fmt::Debug> core::fmt::Debug
    for CoroutineResult<YIELD, RET>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CoroutineResult::Error(e) => f.write_fmt(format_args!("CoroutineResult::Error({})", e)),
            CoroutineResult::Return(r) => {
                f.write_fmt(format_args!("CoroutineResult::Return({:?})", r))
            }
            CoroutineResult::Yield(y) => {
                f.write_fmt(format_args!("CoroutineResult::Yield({:?})", y))
            }
        }
    }
}

#[test]
fn test_simple_coroutine() {
    let mut co = Coroutine::new(
        |co| {
            for i in 0..8u32 {
                let v = co.yield_(i);
                assert!(v == i);
            }
            return 66.98f64;
        },
        (),
    )
    .expect("Coroutine creation failed");

    for i in 0..8u32 {
        let v = co.resume(i);
        assert!(v == Some(CoroutineResult::Yield(i)))
    }

    let v = co.resume(8);
    assert!(v == Some(CoroutineResult::Return(66.98)));
    assert!(co.resume(0).is_none());
}
