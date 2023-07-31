use std::collections::HashMap;

use futures::FutureExt;
use libsqlx::analysis::Statement;
use libsqlx::program::Program;
use libsqlx::query::{Params, Query, Value};
use libsqlx::DescribeResponse;

use super::error::HranaError;
use super::result_builder::SingleStatementBuilder;
use super::{proto, ProtocolError, Version};
use crate::allocation::ConnectionHandle;
// use crate::auth::Authenticated;
use crate::hrana;

/// An error during execution of an SQL statement.
#[derive(thiserror::Error, Debug)]
pub enum StmtError {
    #[error("SQL string could not be parsed: {source}")]
    SqlParse { source: color_eyre::eyre::Error },
    #[error("SQL string does not contain any statement")]
    SqlNoStmt,
    #[error("SQL string contains more than one statement")]
    SqlManyStmts,
    #[error("Arguments do not match SQL parameters: {source}")]
    ArgsInvalid { source: color_eyre::eyre::Error },
    #[error("Specifying both positional and named arguments is not supported")]
    ArgsBothPositionalAndNamed,

    #[error("Transaction timed out")]
    TransactionTimeout,
    #[error("Server cannot handle additional transactions")]
    TransactionBusy,
    #[error("SQLite error: {message}")]
    SqliteError {
        source: libsqlx::rusqlite::ffi::Error,
        message: String,
    },
    #[error("SQL input error: {message} (at offset {offset})")]
    SqlInputError {
        source: libsqlx::rusqlite::ffi::Error,
        message: String,
        offset: i32,
    },

    #[error("Operation was blocked{}", .reason.as_ref().map(|msg| format!(": {}", msg)).unwrap_or_default())]
    Blocked { reason: Option<String> },
}

pub async fn execute_stmt(
    conn: &ConnectionHandle,
    // auth: Authenticated,
    query: Query,
) -> crate::Result<proto::StmtResult, HranaError> {
    let (builder, ret) = SingleStatementBuilder::new();
    conn.execute(
        Program::from_queries(Some(query)), /*, auth*/
        Box::new(builder),
    )
    .await;
    ret.await
        .map_err(|_| crate::error::Error::ConnectionClosed)?
}

pub async fn describe_stmt(
    db: &ConnectionHandle,
    // auth: Authenticated,
    sql: String,
) -> crate::Result<proto::DescribeResult, HranaError> {
    todo!()
    // match db.describe(sql/*, auth*/).await? {
    //     Ok(describe_response) => Ok(proto_describe_result_from_describe_response(
    //         describe_response,
    //     )),
    //     Err(sqld_error) => match stmt_error_from_sqld_error(sqld_error) {
    //         Ok(stmt_error) => Err(stmt_error)?,
    //         Err(sqld_error) => Err(sqld_error)?,
    //     },
    // }
}

pub fn proto_stmt_to_query(
    proto_stmt: &proto::Stmt,
    sqls: &HashMap<i32, String>,
    verion: Version,
) -> crate::Result<Query, HranaError> {
    let sql = proto_sql_to_sql(proto_stmt.sql.as_deref(), proto_stmt.sql_id, sqls, verion)?;

    let mut stmt_iter = Statement::parse(sql);
    let stmt = match stmt_iter.next() {
        Some(Ok(stmt)) => stmt,
        Some(Err(err)) => Err(StmtError::SqlParse { source: err.into() })?,
        None => Err(StmtError::SqlNoStmt)?,
    };

    if stmt_iter.next().is_some() {
        Err(StmtError::SqlManyStmts)?
    }

    let params = if proto_stmt.named_args.is_empty() {
        let values = proto_stmt.args.iter().map(proto_value_to_value).collect();
        Params::Positional(values)
    } else if proto_stmt.args.is_empty() {
        let values = proto_stmt
            .named_args
            .iter()
            .map(|arg| (arg.name.clone(), proto_value_to_value(&arg.value)))
            .collect();
        Params::Named(values)
    } else {
        Err(StmtError::ArgsBothPositionalAndNamed)?
    };

    let want_rows = proto_stmt.want_rows.unwrap_or(true);
    Ok(Query {
        stmt,
        params,
        want_rows,
    })
}

pub fn proto_sql_to_sql<'s>(
    proto_sql: Option<&'s str>,
    proto_sql_id: Option<i32>,
    sqls: &'s HashMap<i32, String>,
    verion: Version,
) -> Result<&'s str, ProtocolError> {
    if proto_sql_id.is_some() && verion < Version::Hrana2 {
        return Err(ProtocolError::NotSupported {
            what: "`sql_id`",
            min_version: Version::Hrana2,
        });
    }

    match (proto_sql, proto_sql_id) {
        (Some(sql), None) => Ok(sql),
        (None, Some(sql_id)) => match sqls.get(&sql_id) {
            Some(sql) => Ok(sql),
            None => Err(ProtocolError::SqlNotFound { sql_id }),
        },
        (Some(_), Some(_)) => Err(ProtocolError::SqlIdAndSqlGiven),
        (None, None) => Err(ProtocolError::SqlIdOrSqlNotGiven),
    }
}

fn proto_value_to_value(proto_value: &proto::Value) -> Value {
    match proto_value {
        proto::Value::Null => Value::Null,
        proto::Value::Integer { value } => Value::Integer(*value),
        proto::Value::Float { value } => Value::Real(*value),
        proto::Value::Text { value } => Value::Text(value.as_ref().into()),
        proto::Value::Blob { value } => Value::Blob(value.as_ref().into()),
    }
}

fn proto_value_from_value(value: Value) -> proto::Value {
    match value {
        Value::Null => proto::Value::Null,
        Value::Integer(value) => proto::Value::Integer { value },
        Value::Real(value) => proto::Value::Float { value },
        Value::Text(value) => proto::Value::Text {
            value: value.into(),
        },
        Value::Blob(value) => proto::Value::Blob {
            value: value.into(),
        },
    }
}

fn proto_describe_result_from_describe_response(
    response: DescribeResponse,
) -> proto::DescribeResult {
    proto::DescribeResult {
        params: response
            .params
            .into_iter()
            .map(|p| proto::DescribeParam { name: p.name })
            .collect(),
        cols: response
            .cols
            .into_iter()
            .map(|c| proto::DescribeCol {
                name: c.name,
                decltype: c.decltype,
            })
            .collect(),
        is_explain: response.is_explain,
        is_readonly: response.is_readonly,
    }
}

impl From<crate::error::Error> for HranaError {
    fn from(error: crate::error::Error) -> Self {
        if let crate::error::Error::Libsqlx(e) = error {
            match e {
                libsqlx::error::Error::LibSqlInvalidQueryParams(source) => StmtError::ArgsInvalid {
                    source: color_eyre::eyre::anyhow!("{source}"),
                }
                .into(),
                libsqlx::error::Error::LibSqlTxTimeout => StmtError::TransactionTimeout.into(),
                libsqlx::error::Error::LibSqlTxBusy => StmtError::TransactionBusy.into(),
                libsqlx::error::Error::Blocked(reason) => StmtError::Blocked { reason }.into(),
                libsqlx::error::Error::RusqliteError(rusqlite_error) => match rusqlite_error {
                    libsqlx::error::RusqliteError::SqliteFailure(sqlite_error, Some(message)) => {
                        StmtError::SqliteError {
                            source: sqlite_error,
                            message,
                        }
                        .into()
                    }
                    libsqlx::error::RusqliteError::SqliteFailure(sqlite_error, None) => {
                        StmtError::SqliteError {
                            message: sqlite_error.to_string(),
                            source: sqlite_error,
                        }
                        .into()
                    }
                    libsqlx::error::RusqliteError::SqlInputError {
                        error: sqlite_error,
                        msg: message,
                        offset,
                        ..
                    } => StmtError::SqlInputError {
                        source: sqlite_error,
                        message,
                        offset,
                    }
                    .into(),
                    rusqlite_error => {
                        return crate::error::Error::from(libsqlx::error::Error::RusqliteError(
                            rusqlite_error,
                        ))
                        .into()
                    }
                },
                sqld_error => return crate::error::Error::from(sqld_error).into(),
            }
        } else {
            Self::Libsqlx(error)
        }
    }
}

pub fn proto_error_from_stmt_error(error: &StmtError) -> hrana::proto::Error {
    hrana::proto::Error {
        message: error.to_string(),
        code: error.code().into(),
    }
}

impl StmtError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SqlParse { .. } => "SQL_PARSE_ERROR",
            Self::SqlNoStmt => "SQL_NO_STATEMENT",
            Self::SqlManyStmts => "SQL_MANY_STATEMENTS",
            Self::ArgsInvalid { .. } => "ARGS_INVALID",
            Self::ArgsBothPositionalAndNamed => "ARGS_BOTH_POSITIONAL_AND_NAMED",
            Self::TransactionTimeout => "TRANSACTION_TIMEOUT",
            Self::TransactionBusy => "TRANSACTION_BUSY",
            Self::SqliteError { source, .. } => sqlite_error_code(source.code),
            Self::SqlInputError { .. } => "SQL_INPUT_ERROR",
            Self::Blocked { .. } => "BLOCKED",
        }
    }
}

fn sqlite_error_code(code: libsqlx::error::ErrorCode) -> &'static str {
    match code {
        libsqlx::error::ErrorCode::InternalMalfunction => "SQLITE_INTERNAL",
        libsqlx::error::ErrorCode::PermissionDenied => "SQLITE_PERM",
        libsqlx::error::ErrorCode::OperationAborted => "SQLITE_ABORT",
        libsqlx::error::ErrorCode::DatabaseBusy => "SQLITE_BUSY",
        libsqlx::error::ErrorCode::DatabaseLocked => "SQLITE_LOCKED",
        libsqlx::error::ErrorCode::OutOfMemory => "SQLITE_NOMEM",
        libsqlx::error::ErrorCode::ReadOnly => "SQLITE_READONLY",
        libsqlx::error::ErrorCode::OperationInterrupted => "SQLITE_INTERRUPT",
        libsqlx::error::ErrorCode::SystemIoFailure => "SQLITE_IOERR",
        libsqlx::error::ErrorCode::DatabaseCorrupt => "SQLITE_CORRUPT",
        libsqlx::error::ErrorCode::NotFound => "SQLITE_NOTFOUND",
        libsqlx::error::ErrorCode::DiskFull => "SQLITE_FULL",
        libsqlx::error::ErrorCode::CannotOpen => "SQLITE_CANTOPEN",
        libsqlx::error::ErrorCode::FileLockingProtocolFailed => "SQLITE_PROTOCOL",
        libsqlx::error::ErrorCode::SchemaChanged => "SQLITE_SCHEMA",
        libsqlx::error::ErrorCode::TooBig => "SQLITE_TOOBIG",
        libsqlx::error::ErrorCode::ConstraintViolation => "SQLITE_CONSTRAINT",
        libsqlx::error::ErrorCode::TypeMismatch => "SQLITE_MISMATCH",
        libsqlx::error::ErrorCode::ApiMisuse => "SQLITE_MISUSE",
        libsqlx::error::ErrorCode::NoLargeFileSupport => "SQLITE_NOLFS",
        libsqlx::error::ErrorCode::AuthorizationForStatementDenied => "SQLITE_AUTH",
        libsqlx::error::ErrorCode::ParameterOutOfRange => "SQLITE_RANGE",
        libsqlx::error::ErrorCode::NotADatabase => "SQLITE_NOTADB",
        libsqlx::error::ErrorCode::Unknown => "SQLITE_UNKNOWN",
        _ => "SQLITE_UNKNOWN",
    }
}

impl From<&proto::Value> for Value {
    fn from(proto_value: &proto::Value) -> Value {
        proto_value_to_value(proto_value)
    }
}

impl From<Value> for proto::Value {
    fn from(value: Value) -> proto::Value {
        proto_value_from_value(value)
    }
}
