"""Measure radius approximation at bin boundaries using public numeric output."""
from probe import *
import re
table=np.array([int(x,16) for x in re.findall('0x[0-9a-f]+',(Path(__file__).parents[1]/'src'/'sqrt_table.rs').read_text())],np.uint32).view(np.float32)
db,lens=synthetic('<distortion model="ptlens" focal="50" c="1"/>',aspect='1')
m=lf.Modifier(lens,1,3,3);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=8)
errors=[];equal=[]
for i in range(1,4096):
    r2=np.uint32(0x3f800000+(i<<12)).view(np.float32)
    yu=np.float32(1+np.sqrt(float(r2)-1));y=np.float32(yu-1)
    rr=np.float32(1+y*y);idx=(int(rr.view(np.uint32))-0x3f800000)>>12
    a=m.apply_geometry_distortion(xu=2,yu=float(yu),width=4,height=1)[0,0,0]-1
    if abs(a-table[idx])>1e-6:errors.append((i,float(rr),float(a),float(table[idx]),int(rr.view(np.uint32))&4095))
    if int(rr.view(np.uint32))&4095==0:equal.append((i,float(a),float(table[idx])))
print('errors',len(errors),errors[:30]);print('exact boundaries',len(equal),'different',sum(abs(a-b)>1e-6 for _,a,b in equal))
