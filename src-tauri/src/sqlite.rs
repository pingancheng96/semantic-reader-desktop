use serde_json::{Map, Value};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    path::{Path, PathBuf},
    ptr,
};

pub type Result<T> = std::result::Result<T, String>;
type Handle = *mut c_void;

#[cfg_attr(not(windows), link(name = "sqlite3"))]
extern "C" {
    fn sqlite3_open(filename: *const c_char, db: *mut Handle) -> c_int;
    fn sqlite3_close(db: Handle) -> c_int;
    fn sqlite3_errmsg(db: Handle) -> *const c_char;
    fn sqlite3_exec(
        db: Handle,
        sql: *const c_char,
        callback: Handle,
        arg: Handle,
        error: Handle,
    ) -> c_int;
    fn sqlite3_prepare_v2(
        db: Handle,
        sql: *const c_char,
        len: c_int,
        stmt: *mut Handle,
        tail: Handle,
    ) -> c_int;
    fn sqlite3_finalize(stmt: Handle) -> c_int;
    fn sqlite3_bind_parameter_count(stmt: Handle) -> c_int;
    fn sqlite3_bind_text(
        stmt: Handle,
        index: c_int,
        text: *const c_char,
        len: c_int,
        free: Handle,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: Handle, index: c_int, value: i64) -> c_int;
    fn sqlite3_bind_null(stmt: Handle, index: c_int) -> c_int;
    fn sqlite3_step(stmt: Handle) -> c_int;
    fn sqlite3_column_count(stmt: Handle) -> c_int;
    fn sqlite3_column_name(stmt: Handle, index: c_int) -> *const c_char;
    fn sqlite3_column_type(stmt: Handle, index: c_int) -> c_int;
    fn sqlite3_column_text(stmt: Handle, index: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: Handle, index: c_int) -> c_int;
    fn sqlite3_column_int64(stmt: Handle, index: c_int) -> i64;
    fn sqlite3_column_double(stmt: Handle, index: c_int) -> f64;
    fn sqlite3_changes(db: Handle) -> c_int;
}

pub struct Db(Handle, PathBuf);
struct Statement(Handle);
impl Drop for Db {
    fn drop(&mut self) {
        unsafe {
            sqlite3_close(self.0);
        }
    }
}
impl Drop for Statement {
    fn drop(&mut self) {
        unsafe {
            sqlite3_finalize(self.0);
        }
    }
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let filename = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        let mut handle = ptr::null_mut();
        let code = unsafe { sqlite3_open(filename.as_ptr(), &mut handle) };
        if handle.is_null() {
            return Err("无法打开 SQLite".into());
        }
        let db = Self(handle, path.to_owned());
        db.check(code)?;
        db.batch("PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;")?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.1
    }

    fn check(&self, code: c_int) -> Result<()> {
        if code == 0 {
            return Ok(());
        }
        Err(unsafe { CStr::from_ptr(sqlite3_errmsg(self.0)) }
            .to_string_lossy()
            .into_owned())
    }

    pub fn batch(&self, sql: &str) -> Result<()> {
        let sql = CString::new(sql).map_err(|e| e.to_string())?;
        self.check(unsafe {
            sqlite3_exec(
                self.0,
                sql.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        })
    }

    pub fn query(&self, sql: &str, args: &[Value]) -> Result<Vec<Value>> {
        let sql = CString::new(sql).map_err(|e| e.to_string())?;
        let mut handle = ptr::null_mut();
        self.check(unsafe {
            sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut handle, ptr::null_mut())
        })?;
        // Text bindings borrow from args, which outlives the statement.
        let stmt = Statement(handle);
        if unsafe { sqlite3_bind_parameter_count(stmt.0) } as usize != args.len() {
            return Err("SQL 参数数量错误".into());
        }
        for (i, arg) in args.iter().enumerate() {
            let index = (i + 1) as c_int;
            self.check(unsafe {
                match arg {
                    Value::String(s) => sqlite3_bind_text(
                        stmt.0,
                        index,
                        s.as_ptr().cast(),
                        s.len() as c_int,
                        ptr::null_mut(),
                    ),
                    Value::Number(n) if n.is_i64() => {
                        sqlite3_bind_int64(stmt.0, index, n.as_i64().unwrap())
                    }
                    Value::Null => sqlite3_bind_null(stmt.0, index),
                    _ => return Err("SQL 参数类型错误".into()),
                }
            })?;
        }
        let mut rows = Vec::new();
        loop {
            match unsafe { sqlite3_step(stmt.0) } {
                101 => return Ok(rows),
                100 => {
                    let mut row = Map::new();
                    for i in 0..unsafe { sqlite3_column_count(stmt.0) } {
                        let (name, value) = unsafe {
                            let name = CStr::from_ptr(sqlite3_column_name(stmt.0, i))
                                .to_string_lossy()
                                .into_owned();
                            let value = match sqlite3_column_type(stmt.0, i) {
                                1 => Value::from(sqlite3_column_int64(stmt.0, i)),
                                2 => Value::from(sqlite3_column_double(stmt.0, i)),
                                3 => {
                                    let data = sqlite3_column_text(stmt.0, i);
                                    let len = sqlite3_column_bytes(stmt.0, i) as usize;
                                    let bytes = if len == 0 || data.is_null() {
                                        &[]
                                    } else {
                                        std::slice::from_raw_parts(data, len)
                                    };
                                    Value::from(String::from_utf8_lossy(bytes).into_owned())
                                }
                                _ => Value::Null,
                            };
                            (name, value)
                        };
                        row.insert(name, value);
                    }
                    rows.push(Value::Object(row));
                }
                code => self.check(code)?,
            }
        }
    }

    pub fn one(&self, sql: &str, args: &[Value]) -> Result<Value> {
        self.query(sql, args)?
            .into_iter()
            .next()
            .ok_or_else(|| "记录不存在，请刷新后重试".into())
    }

    pub fn write(&self, sql: &str, args: &[Value]) -> Result<()> {
        self.query(sql, args)?;
        if unsafe { sqlite3_changes(self.0) } == 0 {
            return Err("记录不存在或已被修改".into());
        }
        Ok(())
    }

    pub fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transact("BEGIN IMMEDIATE", f)
    }

    pub fn snapshot<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transact("BEGIN", f)
    }

    fn transact<T>(&self, begin: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.batch(begin)?;
        match f().and_then(|value| {
            self.batch("COMMIT")?;
            Ok(value)
        }) {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = self.batch("ROLLBACK");
                Err(error)
            }
        }
    }
}
