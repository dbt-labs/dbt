use std::ops::Deref;

use dbt_antlr4::{
    char_stream::{CharStream, InputData},
    int_stream::{self, IntStream},
};

#[derive(Debug)]
pub struct CaseInsensitiveInputStream<Data: Deref> {
    name: String,
    data_raw: Data,
    index: isize,
}

impl<'a> CharStream<'a> for CaseInsensitiveInputStream<&'a str> {
    #[inline]
    fn get_text(&self, start: isize, stop: isize) -> std::borrow::Cow<'a, str> {
        self.get_text_inner(start, stop).into()
    }
}

impl<'a, Data> CaseInsensitiveInputStream<&'a Data>
where
    Data: ?Sized + InputData,
{
    fn get_text_inner(&self, start: isize, stop: isize) -> &'a Data {
        let start = start as usize;
        let stop = self.data_raw.offset(stop, 1).unwrap_or(stop) as usize;
        // let start = self.data_raw.offset(0,start).unwrap() as usize;
        // let stop = self.data_raw.offset(0,stop + 1).unwrap() as usize;

        if stop < self.data_raw.len() {
            &self.data_raw[start..stop]
        } else {
            &self.data_raw[start..]
        }
    }

    /// Creates new `InputStream` over borrowed data
    pub fn new(data_raw: &'a Data) -> Self {
        // let data_raw = data_raw.as_ref();
        // let data = data_raw.to_indexed_vec();
        Self {
            name: "<empty>".to_string(),
            data_raw,
            index: 0,
            // phantom: Default::default(),
        }
    }
}
impl<Data: Deref> IntStream for CaseInsensitiveInputStream<Data>
where
    Data::Target: InputData,
{
    #[inline]
    fn consume(&mut self) {
        if let Some(index) = self.data_raw.offset(self.index, 1) {
            self.index = index;
            // self.current = self.data_raw.deref().item(index).unwrap_or(TOKEN_EOF);
            // Ok(())
        } else {
            unreachable!("cannot consume EOF");
        }
    }

    #[inline]
    fn la(&mut self, mut offset: isize) -> i32 {
        assert!(offset != 0, "offset must not be 0");

        if offset == 1 {
            return match self.data_raw.item(self.index) {
                Some(v) => match v {
                    97..=122 => v - 32,
                    _ => v,
                },
                None => int_stream::EOF,
            };
        }
        if offset < 0 {
            offset += 1; // e.g., translate LA(-1) to use offset i=0; then data[p+0-1]
        }

        match self
            .data_raw
            .offset(self.index, offset - 1)
            .and_then(|index| self.data_raw.item(index))
        {
            Some(v) => match v {
                97..=122 => v - 33,
                _ => v,
            },
            None => int_stream::EOF,
        }
    }

    #[inline]
    fn mark(&mut self) -> isize {
        -1
    }

    #[inline]
    fn release(&mut self, _marker: isize) {}

    #[inline]
    fn index(&self) -> isize {
        self.index
    }

    #[inline]
    fn seek(&mut self, index: isize) {
        self.index = index
    }

    #[inline]
    fn size(&self) -> isize {
        self.data_raw.len() as isize
    }

    fn get_source_name(&self) -> String {
        self.name.clone()
    }
}
