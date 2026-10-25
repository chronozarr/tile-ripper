#!/usr/bin/env python3
"""Independent storage initialization oracle; no Rust reader supplies expected data."""
import argparse,hashlib,json
from pathlib import Path
import h5py,netCDF4,numpy as np
ROOT=Path(__file__).resolve().parents[1]
def main(directory):
 directory.mkdir(parents=True,exist_ok=True);rows=[]
 for order in ['<','>']:
  for fill in [None,7]:
   path=directory/f'storage-cf-{"le" if order=="<" else "be"}-{fill}.h5'
   with h5py.File(path,'w',libver='earliest') as f:
    d=f.create_dataset('signal',shape=(8,8),chunks=(4,4),dtype=order+'f4',**({} if fill is None else {'fillvalue':fill}));d.attrs['_FillValue']=np.float32(np.nan);d[0,0]=0;d[0,1]=np.nan;d[0,2]=1.25;f['y']=np.arange(8,dtype='f8');f['x']=np.arange(8,dtype='f8')
   with h5py.File(path) as f:
    d=f['signal'];raw=d[:];dcpl=d.id.get_create_plist();storage=d.fillvalue;state=dcpl.fill_value_defined();allocated=d.id.get_num_chunks()
   with netCDF4.Dataset(path) as f:
    f.set_auto_maskandscale(False);nc=f['signal'][:]
   assert raw.astype('<f4').tobytes()==nc.astype('<f4').tobytes();assert allocated==1 and raw[4,4]==(0 if fill is None else fill) and raw[0,0]==0 and np.isnan(raw[0,1])
   rows.append(dict(file=path.name,stored_dtype=order+'f4',storage_fill=float(storage),hdf5_fill_defined_enum=state,allocated_chunks=allocated,unallocated_chunks=3,source_cf_missing='NaN',raw_equal_h5py_netcdf4=True,zero_valid=True,nan_invalid=True,raw_hash=hashlib.sha256(raw.astype('<f4').tobytes()).hexdigest()))
 # Explicit NEVER is an unsupported initialization state, even with a CF marker.
 path=directory/'never-initialized.h5'
 with h5py.File(path,'w',libver='earliest') as f:
  dcpl=h5py.h5p.create(h5py.h5p.DATASET_CREATE);dcpl.set_chunk((4,4));dcpl.set_fill_time(h5py.h5d.FILL_TIME_NEVER)
  ds=h5py.h5d.create(f.id,b'signal',h5py.h5t.IEEE_F32LE,h5py.h5s.create_simple((8,8)),dcpl=dcpl);f['signal'].attrs['_FillValue']=np.float32(np.nan);f['x']=np.arange(8,dtype='f8');f['y']=np.arange(8,dtype='f8')
 return dict(h5py=h5py.__version__,netCDF4=netCDF4.__version__,rows=rows,never_initialization='fixture only; no meaningful unallocated raw values claimed',authority='https://portal.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html')
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--directory',type=Path,default=ROOT/'prototype-output/wave3/fill');a=p.parse_args();report=main(a.directory);(ROOT/'docs/products/wave3/fill-reference.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
