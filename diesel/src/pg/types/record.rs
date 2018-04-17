use expression::{AppearsOnTable, AsExpression, Expression, NonAggregate, SelectableExpression};
use byteorder::{NetworkEndian, ReadBytesExt};
use pg::Pg;
use deserialize::{self, FromSql, FromSqlRow, Queryable};
use sql_types::Record;
use row::Row;
use result::QueryResult;
use query_builder::{AstPass, QueryFragment};

macro_rules! tuple_impls {
    ($(
        $Tuple:tt {
            $(($idx:tt) -> $T:ident, $ST:ident, $TT:ident,)+
        }
    )+) => {$(
        impl<$($T,)+ $($ST,)+> FromSql<Record<($($ST,)+)>, Pg> for ($($T,)+)
        where
            $($T: FromSql<$ST, Pg>,)+
        {
            // Yes, we're relying on the order of evaluation of subexpressions
            // but the only other option would be to use `mem::uninitialized`
            // and `ptr::write`.
            #[cfg_attr(feature = "clippy", allow(eval_order_dependence))]
            fn from_sql(bytes: Option<&[u8]>) -> deserialize::Result<Self> {
                let mut bytes = not_none!(bytes);
                let num_elements = bytes.read_i32::<NetworkEndian>()?;

                if num_elements != $Tuple {
                    return Err(format!(
                        "Expected a tuple of {} elements, got {}",
                        $Tuple,
                        num_elements,
                    ).into());
                }

                let result = ($({
                    // We could in theory validate the OID here, but that
                    // ignores cases like text vs varchar where the
                    // representation is the same and we don't care which we
                    // got.
                    let _oid = bytes.read_u32::<NetworkEndian>()?;
                    let num_bytes = bytes.read_i32::<NetworkEndian>()?;

                    if num_bytes == -1 {
                        $T::from_sql(None)?
                    } else {
                        let (elem_bytes, new_bytes) = bytes.split_at(num_bytes as usize);
                        bytes = new_bytes;
                        $T::from_sql(Some(elem_bytes))?
                    }
                },)+);

                if bytes.is_empty() {
                    Ok(result)
                } else {
                    Err("Received too many bytes. This tuple likely contains \
                        an element of the wrong SQL type.".into())
                }
            }
        }

        impl<$($T,)+ $($ST,)+> FromSqlRow<Record<($($ST,)+)>, Pg> for ($($T,)+)
        where
            Self: FromSql<Record<($($ST,)+)>, Pg>,
        {
            const FIELDS_NEEDED: usize = 1;

            fn build_from_row<RowT: Row<Pg>>(row: &mut RowT) -> deserialize::Result<Self> {
                Self::from_sql(row.take())
            }
        }

        impl<$($T,)+ $($ST,)+> Queryable<Record<($($ST,)+)>, Pg> for ($($T,)+)
        where
            Self: FromSqlRow<Record<($($ST,)+)>, Pg>,
        {
            type Row = Self;

            fn build(row: Self::Row) -> Self {
                row
            }
        }

        impl<$($T,)+ $($ST,)+> AsExpression<Record<($($ST,)+)>> for ($($T,)+)
        where
            $($T: AsExpression<$ST>,)+
            PgTuple<($($T::Expression,)+)>: Expression<SqlType = Record<($($ST,)+)>>,
        {
            type Expression = PgTuple<($($T::Expression,)+)>;

            fn as_expression(self) -> Self::Expression {
                PgTuple(($(
                    self.$idx.as_expression(),
                )+))
            }
        }
    )+}
}

__diesel_for_each_tuple!(tuple_impls);

#[derive(Debug, Clone, Copy, QueryId)]
pub struct PgTuple<T>(T);

impl<T> QueryFragment<Pg> for PgTuple<T>
where
    T: QueryFragment<Pg>,
{
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.push_sql("(");
        self.0.walk_ast(out.reborrow())?;
        out.push_sql(")");
        Ok(())
    }
}

impl<T> Expression for PgTuple<T>
where
    T: Expression,
{
    type SqlType = Record<T::SqlType>;
}

impl<T, QS> SelectableExpression<QS> for PgTuple<T>
where
    T: SelectableExpression<QS>,
    Self: AppearsOnTable<QS>,
{
}

impl<T, QS> AppearsOnTable<QS> for PgTuple<T>
where
    T: AppearsOnTable<QS>,
    Self: Expression,
{
}

impl<T> NonAggregate for PgTuple<T>
where
    T: NonAggregate,
    Self: Expression,
{
}

#[cfg(test)]
mod tests {
    use test_helpers::*;
    use sql_types::*;
    use dsl::sql;
    use prelude::*;

    #[test]
    fn record_deserializes_correctly() {
        let conn = pg_connection();

        let tup =
            sql::<Record<(Integer, Text)>>("SELECT (1, 'hi')").get_result::<(i32, String)>(&conn);
        assert_eq!(Ok((1, String::from("hi"))), tup);

        let tup = sql::<Record<(Record<(Integer, Text)>, Integer)>>("SELECT ((2, 'bye'), 3)")
            .get_result::<((i32, String), i32)>(&conn);
        assert_eq!(Ok(((2, String::from("bye")), 3)), tup);

        let tup = sql::<Record<(Record<(Nullable<Integer>, Nullable<Text>)>, Nullable<Integer>)>>("SELECT ((4, NULL), NULL)")
            .get_result::<((Option<i32>, Option<String>), Option<i32>)>(&conn);
        assert_eq!(Ok(((Some(4), None), None)), tup);
    }

    #[test]
    fn record_kinda_sorta_not_really_serializes_correctly() {
        let conn = pg_connection();

        let tup = sql::<Record<(Integer, Text)>>("(1, 'hi')");
        let res = ::select(tup.eq((1, "hi"))).get_result(&conn);
        assert_eq!(Ok(true), res);

        let tup = sql::<Record<(Record<(Integer, Text)>, Integer)>>("((2, 'bye'::text), 3)");
        let res = ::select(tup.eq(((2, "bye"), 3))).get_result(&conn);
        assert_eq!(Ok(true), res);

        let tup = sql::<
            Record<
                (
                    Record<(Nullable<Integer>, Nullable<Text>)>,
                    Nullable<Integer>,
                ),
            >,
        >("((4, NULL::text), NULL::int4)");
        let res = ::select(tup.is_not_distinct_from(((Some(4), None::<&str>), None::<i32>)))
            .get_result(&conn);
        assert_eq!(Ok(true), res);
    }
}