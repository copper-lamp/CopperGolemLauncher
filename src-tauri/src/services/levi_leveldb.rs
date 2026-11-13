//! Bounded read-only LevelDB snapshot decoder for Chromium Local Storage.
//! Never opens a DB engine/LOCK, repairs files, or reads obsolete tables.
use std::{collections::{BTreeMap,BTreeSet}, path::{Path,PathBuf}, time::{Duration,Instant}};
type R<T> = Result<T,String>;
const LIMIT:usize=32*1024*1024;
const MAGIC:u64=0xdb4775248b80fb57;
struct Reader { remaining:usize, until:Instant, observed:Vec<(PathBuf,Vec<u8>)> }
impl Reader {
    fn check(&self)->R<()> {if Instant::now()>self.until {Err("Levi preference read exceeded time budget".into())}else{Ok(())}}
    fn file(&mut self,p:&Path)->R<Vec<u8>> {
        self.check()?;
        if self.observed.len()>=64{return Err("Levi database has too many live files".into());}
        let m=std::fs::symlink_metadata(p).map_err(|e|e.to_string())?;
        #[cfg(windows)] {use std::os::windows::fs::MetadataExt;if m.file_attributes()&0x400!=0{return Err("Levi database file is a reparse point".into());}}
        if !m.is_file()||m.file_type().is_symlink()||m.len()>self.remaining as u64{return Err("Levi database file exceeds read budget or is not a regular file".into());}
        use std::io::Read;
        let mut data=Vec::new();std::fs::File::open(p).map_err(|e|format!("Cannot read Levi database: {e}"))?.take(m.len()+1).read_to_end(&mut data).map_err(|e|e.to_string())?;
        if data.len()!=m.len() as usize{return Err("Levi database changed while reading".into());}
        self.remaining-=data.len();self.observed.push((p.to_owned(),data.clone()));Ok(data)
    }
    fn stable(&self)->R<()> {
        // Only selected database files, never the profile. Re-read to detect active compaction/WAL changes.
        for (p,expected) in &self.observed {self.check()?;let m=std::fs::metadata(p).map_err(|e|e.to_string())?;if m.len()!=expected.len() as u64{return Err("Levi is writing preferences; retry after closing it".into());}use std::io::Read;let mut actual=Vec::new();std::fs::File::open(p).map_err(|e|e.to_string())?.take(expected.len()as u64+1).read_to_end(&mut actual).map_err(|e|e.to_string())?;if actual!=*expected{return Err("Levi preferences changed during read; retry after closing it".into());}}
        Ok(())
    }
}
fn var(input:&[u8],p:&mut usize)->R<u64>{let mut n=0u64;for shift in (0..70).step_by(7){let b=*input.get(*p).ok_or("Truncated LevelDB varint")?;*p+=1;if shift==63&&b>1{return Err("LevelDB varint overflow".into());}n|=((b&127)as u64)<<shift;if b<128{return Ok(n);}}Err("Invalid LevelDB varint".into())}
fn bytes<'a>(input:&'a[u8],p:&mut usize)->R<&'a[u8]>{let n=usize::try_from(var(input,p)?).map_err(|_|"LevelDB length overflow")?;let end=p.checked_add(n).ok_or("LevelDB length overflow")?;let out=input.get(*p..end).ok_or("Truncated LevelDB string")?;*p=end;Ok(out)}
fn u32le(b:&[u8])->R<u32>{Ok(u32::from_le_bytes(b.try_into().map_err(|_|"Invalid LevelDB fixed32")?))}
fn u64le(b:&[u8])->R<u64>{Ok(u64::from_le_bytes(b.try_into().map_err(|_|"Invalid LevelDB fixed64")?))}
fn checksum(data:&[u8])->u32 {let mut c=!0u32;for &b in data {c^=b as u32;for _ in 0..8{c=(c>>1)^if c&1!=0{0x82f63b78}else{0};}}let c=!c;c.rotate_right(15).wrapping_add(0xa282ead8)}
fn records(data:&[u8])->R<Vec<Vec<u8>>>{
    let mut out=Vec::new();let mut partial=Vec::new();let mut active=false;let mut p=0;
    while p<data.len(){let block_left=32768-p%32768;if block_left<7{let end=(p+block_left).min(data.len());if data[p..end].iter().any(|x|*x!=0){return Err("Invalid LevelDB log padding".into());}p=end;continue;}
        if data.len()-p<7{return Err("Incomplete LevelDB log header".into());}let crc=u32le(&data[p..p+4])?;let len=u16::from_le_bytes([data[p+4],data[p+5]])as usize;let kind=data[p+6];if kind==0&&len==0&&crc==0{let end=(p+block_left).min(data.len());if data[p..end].iter().any(|x|*x!=0){return Err("Invalid LevelDB empty record".into());}p=end;continue;}
        if len+7>block_left{return Err("LevelDB log record crosses physical block".into());}let payload=data.get(p+7..p+7+len).ok_or("Incomplete LevelDB log payload")?;let mut checked=Vec::with_capacity(len+1);checked.push(kind);checked.extend_from_slice(payload);if checksum(&checked)!=crc{return Err("LevelDB log CRC32C mismatch".into());}p+=7+len;
        match kind {1=>{if active{return Err("Invalid LevelDB fragment sequence".into());}out.push(payload.to_vec());},2=>{if active{return Err("Nested LevelDB log fragment".into());}partial=payload.to_vec();active=true;},3|4=>{if !active{return Err("Orphan LevelDB log fragment".into());}if partial.len()+payload.len()>LIMIT{return Err("LevelDB logical record too large".into());}partial.extend_from_slice(payload);if kind==4{out.push(std::mem::take(&mut partial));active=false;}},_=>return Err("Unknown LevelDB physical record type".into())}
    }if active{return Err("Incomplete LevelDB logical record".into());}Ok(out)
}
fn snappy(data:&[u8])->R<Vec<u8>>{
    let mut p=0;let size=usize::try_from(var(data,&mut p)?).map_err(|_|"Snappy size overflow")?;if size>LIMIT{return Err("Snappy block too large".into());}let mut out=Vec::with_capacity(size);
    while p<data.len(){let tag=data[p];p+=1;match tag&3 {
        0=>{let mut len=(tag>>2)as usize;if len<60{len+=1;}else{let n=len-59;let b=data.get(p..p+n).ok_or("Truncated Snappy literal size")?;len=0;for (i,v) in b.iter().enumerate(){len|=(*v as usize)<<(i*8);}len=len.checked_add(1).ok_or("Snappy literal overflow")?;p+=n;}if len>size.saturating_sub(out.len()){return Err("Snappy output overflow".into());}out.extend_from_slice(data.get(p..p+len).ok_or("Truncated Snappy literal")?);p+=len;},
        kind=>{let (len,offset)=match kind {1=>{let b=*data.get(p).ok_or("Truncated Snappy copy")?;p+=1;(4+((tag>>2)&7)as usize,(((tag as usize)&0xe0)<<3)|b as usize)},2=>{let b=data.get(p..p+2).ok_or("Truncated Snappy copy")?;p+=2;(1+(tag>>2)as usize,u16::from_le_bytes([b[0],b[1]])as usize)},_=>{let b=data.get(p..p+4).ok_or("Truncated Snappy copy")?;p+=4;(1+(tag>>2)as usize,u32le(b)?as usize)}};if offset==0||offset>out.len()||len>size.saturating_sub(out.len()){return Err("Invalid Snappy backreference".into());}for _ in 0..len{let b=out[out.len()-offset];out.push(b);}}
    }}if out.len()!=size{return Err("Snappy output size mismatch".into());}Ok(out)
}
fn handle(data:&[u8],p:&mut usize)->R<(usize,usize)>{let offset=usize::try_from(var(data,p)?).map_err(|_|"Block offset overflow")?;let size=usize::try_from(var(data,p)?).map_err(|_|"Block size overflow")?;Ok((offset,size))}
fn block(file:&[u8],h:(usize,usize))->R<Vec<u8>>{let (offset,size)=h;let end=offset.checked_add(size).ok_or("Block overflow")?;let trailer_end=end.checked_add(5).ok_or("Block overflow")?;let all=file.get(offset..trailer_end).ok_or("Truncated LevelDB block")?;let crc=u32le(&all[size+1..])?;if checksum(&all[..size+1])!=crc{return Err("LevelDB table CRC32C mismatch".into());}match all[size]{0=>Ok(all[..size].to_vec()),1=>snappy(&all[..size]),_=>Err("Unsupported LevelDB compression".into())}}
fn entries(data:&[u8])->R<Vec<(Vec<u8>,Vec<u8>)>>{
    if data.len()<4{return Err("Truncated LevelDB restart array".into());}let n=u32le(&data[data.len()-4..])?as usize;let tail=n.checked_mul(4).and_then(|v|v.checked_add(4)).ok_or("Restart array overflow")?;if tail>data.len()||n==0{return Err("Invalid LevelDB restart count".into());}let end=data.len()-tail;let mut last_restart=0;for i in 0..n{let r=u32le(&data[end+i*4..end+i*4+4])?as usize;if r>end||(i==0&&r!=0)||(i>0&&r<=last_restart){return Err("Invalid LevelDB restart offset".into());}last_restart=r;}
    let mut p=0;let mut key=Vec::new();let mut out=Vec::new();while p<end{let shared=var(&data[..end],&mut p)?as usize;let unshared=var(&data[..end],&mut p)?as usize;let value_len=var(&data[..end],&mut p)?as usize;if shared>key.len(){return Err("Invalid LevelDB shared prefix".into());}let k_end=p.checked_add(unshared).ok_or("Entry overflow")?;let v_end=k_end.checked_add(value_len).ok_or("Entry overflow")?;key.truncate(shared);key.extend_from_slice(data.get(p..k_end).filter(|_|k_end<=end).ok_or("Truncated LevelDB key")?);let value=data.get(k_end..v_end).filter(|_|v_end<=end).ok_or("Truncated LevelDB value")?;out.push((key.clone(),value.to_vec()));p=v_end;if out.len()>200000{return Err("Too many LevelDB entries".into());}}Ok(out)
}
#[derive(Default)]
struct Snapshot{log:u64,prev:u64,tables:BTreeMap<u64,u64>}
fn manifest(data:&[u8])->R<Snapshot>{let mut s=Snapshot::default();for record in records(data)?{let mut p=0;while p<record.len(){match var(&record,&mut p)? {
    1=>{if bytes(&record,&mut p)?!=b"leveldb.BytewiseComparator"{return Err("Unsupported LevelDB comparator".into());}},2=>s.log=var(&record,&mut p)?,3|4=>{var(&record,&mut p)?;},5=>{var(&record,&mut p)?;bytes(&record,&mut p)?;},6=>{var(&record,&mut p)?;s.tables.remove(&var(&record,&mut p)?);},7=>{var(&record,&mut p)?;let number=var(&record,&mut p)?;let size=var(&record,&mut p)?;bytes(&record,&mut p)?;bytes(&record,&mut p)?;s.tables.insert(number,size);},9=>s.prev=var(&record,&mut p)?,_=>return Err("Unsupported LevelDB manifest edit".into())
}}}Ok(s)}
type Values=BTreeMap<Vec<u8>,(u64,Option<Vec<u8>>)>;
fn keep(out:&mut Values,key:&[u8],seq:u64,value:Option<&[u8]>)->R<()>{
    // Chromium records: '_' + origin + NUL + encoded localStorage key.
    let Some(rest)=key.strip_prefix(b"_")else{return Ok(())};let Some(split)=rest.iter().position(|b|*b==0)else{return Ok(())};let origin=&rest[..split];if origin!=b"http://wails.localhost"&&origin!=b"https://wails.localhost"&&origin!=b"wails://wails.localhost"{return Ok(());}
    let name=decode(&rest[split+1..])?;if !matches!(name.as_str(),"app.lightThemeColor"|"app.darkThemeColor"|"app.themeColor"|"app.lightCustomThemeColor"|"app.darkCustomThemeColor"|"app.customThemeColor"){return Ok(());}
    if let Some((old_seq,old))=out.get(key){if *old_seq>seq{return Ok(());}if *old_seq==seq&&old.as_deref()!=value{return Err("Conflicting LevelDB sequence".into());}}
    out.insert(key.to_vec(),(seq,value.map(|v|v.to_vec())));Ok(())
}
fn decode(bytes:&[u8])->R<String>{match bytes.split_first(){Some((1,text))=>Ok(text.iter().map(|b|char::from(*b)).collect()),Some((0,text))if text.len()%2==0=>String::from_utf16(&text.chunks_exact(2).map(|c|u16::from_le_bytes([c[0],c[1]])).collect::<Vec<_>>()).map_err(|_|"Invalid Chromium UTF16".into()),_=>Err("Unsupported Chromium string encoding".into())}}
/// Reads CURRENT, its manifest, active SSTs, and current/previous WAL only.
pub(super) fn preferences(dir:&Path)->R<BTreeMap<String,String>>{
    let mut reader=Reader{remaining:LIMIT,until:Instant::now()+Duration::from_secs(2),observed:Vec::new()};let current=reader.file(&dir.join("CURRENT"))?;let name=std::str::from_utf8(&current).map_err(|_|"Invalid LevelDB CURRENT")?.trim();if !name.starts_with("MANIFEST-")||name.len()<=9||!name[9..].bytes().all(|b|b.is_ascii_digit())||name.len()>40{return Err("Invalid LevelDB manifest name".into());}let s=manifest(&reader.file(&dir.join(name))?)?;let mut values=Values::new();
    for (num,size) in s.tables {reader.check()?;let mut path=dir.join(format!("{num:06}.ldb"));if !path.exists(){path=dir.join(format!("{num:06}.sst"));}let file=reader.file(&path)?;if file.len()as u64!=size||file.len()<48{return Err("Invalid LevelDB table size".into());}let footer=&file[file.len()-48..];if u64le(&footer[40..])?!=MAGIC{return Err("Invalid LevelDB table magic".into());}let mut p=0;let _meta=handle(footer,&mut p)?;let index=handle(footer,&mut p)?;for (_,v) in entries(&block(&file,index)?)? {reader.check()?;let mut p=0;let h=handle(&v,&mut p)?;if p!=v.len(){return Err("Invalid LevelDB index handle".into());}for (key,value) in entries(&block(&file,h)?)? {if key.len()<8{return Err("Invalid LevelDB internal key".into());}let tag=u64le(&key[key.len()-8..])?;match tag&255{0=>keep(&mut values,&key[..key.len()-8],tag>>8,None)?,1=>keep(&mut values,&key[..key.len()-8],tag>>8,Some(&value))?,_=>return Err("Unsupported LevelDB value type".into())}}}}
    let logs:BTreeSet<_>=[s.log,s.prev].into_iter().filter(|v|*v!=0).collect();for num in logs {for record in records(&reader.file(&dir.join(format!("{num:06}.log")))?)?{reader.check()?;if record.len()<12{return Err("Truncated LevelDB write batch".into());}let seq=u64le(&record[..8])?;let count=u32le(&record[8..12])?as usize;if count>200000{return Err("LevelDB write batch too large".into());}let mut p=12;for i in 0..count{let tag=*record.get(p).ok_or("Truncated LevelDB batch entry")?;p+=1;let key=bytes(&record,&mut p)?;let sequence=seq.checked_add(i as u64).ok_or("Sequence overflow")?;match tag{0=>keep(&mut values,key,sequence,None)?,1=>{let value=bytes(&record,&mut p)?;keep(&mut values,key,sequence,Some(value))?;},_=>return Err("Unknown LevelDB batch operation".into())}}if p!=record.len(){return Err("Invalid LevelDB batch count".into());}}}
    reader.stable()?;let mut origins=BTreeMap::<Vec<u8>,BTreeMap<String,String>>::new();for(key,(_,value))in values{if let Some(value)=value{let split=key.iter().position(|b|*b==0).ok_or("Invalid Chromium key")?;origins.entry(key[..split].to_vec()).or_default().insert(decode(&key[split+1..])?,decode(&value)?);}}
    if origins.len()>1{return Err("Multiple Levi origins contain preferences; refusing ambiguous import".into());}Ok(origins.into_values().next().unwrap_or_default())
}
