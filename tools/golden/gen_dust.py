"""Generated dust scans only. The oracle is read-only; run with its interpreter and -B.

--check-rust checks Rust zlib payloads and generated catalog snapshots with Python.
"""
import sys
sys.dont_write_bytecode = True
from common import Case, ROOT, GOLDEN, np, inputs, write_json
import cv2
import hashlib
import json
import math
import zlib
from dataclasses import asdict, replace
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import patch
from luma import dust, dust_shapes as shapes, dust_soft as soft, dust_scratches as scratches, dust_texture as texture, dust_store as store, dust_batch as batch


def checksum(a):
    return hashlib.sha256(np.ascontiguousarray(a).tobytes()).hexdigest()


def check_rust():
    folder = ROOT / 'target' / 'dust-roundtrip'
    for path in sorted(folder.glob('payload_*.zlib')):
        actual = json.loads(store.unpack(path.read_bytes()))
        expected = json.loads(path.with_suffix('.json').read_text(encoding='utf-8'))
        assert actual == expected, path
    from luma.catalog import Catalog
    with Catalog.open_reader(folder / 'catalog') as catalog:
        expected = json.loads((folder / 'batch.json').read_text(encoding='utf-8'))
        assert batch.capture_scope_ids(catalog, 1, [2, 1, 2, 999], None) == expected['scope']
        record = batch.capture_record(catalog, 1)
        assert record == expected['record']
        batch.verify_source(catalog, record)
    print('Python read all Rust payloads and verified the Rust catalog/source snapshot.')


def scenes():
    rng = np.random.default_rng(100321)
    h,w = 128,160
    y,x = np.mgrid[:h,:w].astype(np.float32)
    smooth = (.38 + x*.001 + y*.0004).astype(np.float32)
    for cx,cy,r,value in [(35,35,2,.05),(80,38,4,.95),(118,79,7,.1),(62,92,3,.7),(0,50,4,.02),(158,110,3,.95)]:
        smooth[(x-cx)**2+(y-cy)**2<=r*r] = value
    grain = np.clip(smooth + rng.normal(0,.006,(h,w)).astype(np.float32),0,1)
    pattern = (.5 + .04*np.cos(x*np.float32(math.pi/4))*np.cos(y*np.float32(math.pi/4))).astype(np.float32)
    pattern[(x-77)**2+(y-65)**2<=9] = .1
    blobs = np.full((192,224),.5,np.float32)
    yy,xx = np.mgrid[:192,:224].astype(np.float32)
    for cx,cy,s,amp in [(50,65,2.2,-.13),(135,70,5.5,.18),(95,125,8.5,-.2)]:
        blobs += amp*np.exp(-((xx-cx)**2+(yy-cy)**2)/(2*s*s))
    hairs=np.full((192,240),.5,np.float32)
    cv2.line(hairs,(3,20),(180,25),.08,2)
    cv2.line(hairs,(90,48),(130,145),.92,3)
    cv2.line(hairs,(65,115),(175,70),.92,2)
    cv2.ellipse(hairs,(60,125),(27,35),15,15,290,.1,2)
    cv2.circle(hairs,(195,125),24,.08,2)
    cv2.rectangle(hairs,(190,50),(211,68),.1,-1)
    seams=np.full((700,900),.5,np.float32)
    for cx,cy,r,value in [(637,310,4,.1),(647,510,6,.92),(400,637,3,.04),(785,650,5,.9),(4,4,3,.95)]:
        cv2.circle(seams,(cx,cy),r,value,-1)
    cv2.line(seams,(590,610),(695,670),.12,2)
    edge=np.full((128,160),.35,np.float32);edge[:,80:]=.65
    cv2.circle(edge,(78,64),3,.08,-1);cv2.circle(edge,(95,43),3,.95,-1)
    result={'smooth':smooth,'grain':grain,'pattern':pattern,'soft':blobs,'hairs':hairs,'seams':seams,'edge':edge,'constant':np.full((32,33),.4,np.float32),'tiny':np.full((1,1),.5,np.float32)}
    for name,a in inputs().items():
        result['standard_'+name]=cv2.cvtColor(a,cv2.COLOR_RGB2GRAY)
    return result


def detection(data):
    variants=[]
    base=dust.Options()
    for name in data:variants.append((name,base,640,500))
    # Every numeric/string option has default plus two alternatives; booleans have both legal values.
    changes=[{'sensitivity':1},{'sensitivity':100},{'minimum':2},{'minimum':12}, {'maximum':8},{'maximum':160}, {'polarity':'dark'},{'polarity':'bright'}, {'detailed':True}, {'reduce_patterns':True}, {'soft':True}, {'suppress_grain':True}, {'scratches':True}, {'scratch_width':2},{'scratch_width':40}]
    for change in changes:variants.append(('grain',replace(base,**change),640,500))
    for name in ['smooth','grain','pattern','soft','edge']:
        for reduce,suppress in [(False,False),(True,False),(True,True)]:
            variants.append((name,replace(base,detailed=True,soft=True,reduce_patterns=reduce,suppress_grain=suppress,sensitivity=85),640,500))
    for width in [2,12,40]:
        for reduce in [False,True]:variants.append(('hairs',replace(base,scratches=True,scratch_width=width,reduce_patterns=reduce,sensitivity=80),96,500))
    variants += [('smooth',replace(base,sensitivity=100),32,1),('grain',replace(base,detailed=True),48,2),('seams',replace(base,scratches=True,sensitivity=80),640,3),('soft',replace(base,detailed=True,soft=True,scratches=True),96,1)]
    meta=[]
    for i,(name,opt,tile,limit) in enumerate(variants):
        events=[];result=dust.detect(data[name],opt,progress=lambda a,b:events.append([a,b]),tile_size=tile,limit=limit);result.pop('seconds')
        meta.append(dict(name=name,options=asdict(opt),tile_size=tile,limit=limit,result=result,progress=events))
        print('detect',i,name,len(result['spots']),flush=True)
    with Case('dust','detect') as c:c.json(cases=meta)
    with Case('dust','candidates') as c:
        cases=[]
        for name,module,opts in [('soft',soft,replace(base,detailed=True,soft=True,sensitivity=90)),('grain',soft,replace(base,detailed=True,soft=True,suppress_grain=True,sensitivity=90)),('hairs',scratches,replace(base,scratches=True,sensitivity=80)),('pattern',scratches,replace(base,scratches=True,reduce_patterns=True))]:
            events=[];out=module.candidates(data[name],opts,tile_size=96,progress=lambda a,b:events.append([a,b]))
            cases.append(dict(name=name,function='soft' if module is soft else 'scratches',options=asdict(opts),tile_size=96,output=out,progress=events))
        c.json(cases=cases)
    return meta


class CancelAfter:
    def __init__(self,n):self.n=n;self.calls=0
    def is_set(self):self.calls+=1;return self.calls>=self.n


def helpers(data):
    rng=np.random.default_rng(1729)
    with Case('dust','helpers') as c:
        vectors=[np.array([1],np.float32),np.array([2,1],np.float32),np.array([.1,.2,.9,-1,3,0],np.float32),rng.normal(size=257).astype(np.float32),np.array([16777216,16777218],np.float32)]
        fractions=[0,.1,.15,.5,.6,.8,.85,.9,1]
        records=[]
        for i,v in enumerate(vectors):
            c.array(f'vector_{i}',v);plan=dust._quantile_plan(len(v),tuple(fractions));records.append(dict(median=dust._median(v),quantiles=dust._quantiles(v,*fractions),plan=[v.tolist() for v in plan]))
        stats=np.array([[0,0,100,100,9000],[10,10,4,4,16],[30,20,8,2,13],[12,15,5,4,6],[2,2,0,0,0],[7,7,30,20,450],[99,99,3,3,7]],np.int32)
        c.array('stats',stats)
        c.json(vectors=records,fractions=fractions,percentile_dtype=str(dust._PERCENTILE_DTYPE),components=[dict(args=a,output=dust._component_indices(stats,*a)) for a in [(2,40,0,0,0,0,100,100),(3,8,10,20,15,25,110,120),(20,160,0,0,0,0,100,100)]])
        repeated=[]
        for name,rect in [('pattern',[71,59,13,13]),('pattern',[30,30,5,5]),('constant',[0,0,2,2]),('smooth',[76,34,9,9])]:
            for reduce in [False,True]:repeated.append(dict(name=name,rect=rect,reduce=reduce,output=dust._repeated_detail(data[name],*rect,reduce_patterns=reduce)))
        c.json(repeated=repeated)
    with Case('dust','shapes') as c:
        shape=(51,73); records=[]
        simple=[dict(x=.5,y=.5,rx=.1,ry=.04),dict(x=0,y=1,rx=.01,ry=.01,scale=25),dict(x=.9,y=.1,rx=.15,ry=.2,scale=250)]
        manual=[]
        for points,diameter in [([[.5,.5]],1),([[0,0],[1,1]],6),([[.15,.4],[.4,.8],[.8,.3]],12),([[.2,.2],[.2,.2]],160)]:
            spot=shapes.manual(points,shape,diameter);manual.append(dict(points=points,diameter=diameter,spot=spot));simple.append(spot)
        for i,spot in enumerate(simple):
            parts=shapes.ellipses(spot,shape);mask=np.zeros(shape,np.uint8);shapes.paint(mask,parts);c.array(f'mask_{i}',mask)
            crop=np.full((21,27),17,np.uint8);shapes.paint(crop,parts,13,9);c.array(f'crop_{i}',crop)
            records.append(dict(spot=spot,spot_bounds=dust.spot_bounds(spot,shape),parts=parts,bounds=shapes.bounds(parts,shape)))
        made=shapes.make_spot([(5.,10.,2.,3.),(14.5,18.5,4.,2.)],shape,polarity='manual',score=0)
        c.json(shape=shape,records=records,manual=manual,made=made)
    with Case('dust','cover') as c:
        cases=[]
        for i,kind in enumerate(['line','cross','loop','empty','full','overflow']):
            a=np.zeros((96,96) if kind=='overflow' else (37,53),np.uint8)
            if kind=='line':cv2.line(a,(0,5),(45,31),1,2)
            if kind=='cross':cv2.line(a,(4,18),(48,18),1,3);cv2.line(a,(25,2),(25,34),1,2)
            if kind=='loop':cv2.circle(a,(26,18),12,1,2)
            if kind=='full':a[:]=1
            if kind=='overflow':a[::2,::2]=1
            d=np.zeros(a.shape,np.float32) if kind=='overflow' else cv2.distanceTransform(np.pad(a,1),cv2.DIST_L2,5)[1:-1,1:-1]
            c.array(f'mask_{i}',a);c.array(f'distance_{i}',d);cases.append(dict(origin=[7,11],parts=scratches.cover(a,d,7,11)))
        c.json(cases=cases)
    with Case('dust','merge') as c:
        existing=[dict(x=.4,y=.5,rx=.1,ry=.08,polarity='dark',score=2),dict(x=.7,y=.7,rx=.05,ry=.05,polarity='bright',score=3)]
        additional=[dict(x=x,y=y,rx=.08,ry=.08,polarity=p,score=s) for x,y,p,s in [(.4,.5,'dark',9),(.41,.5,'bright',5),(.7,.7,'bright',8),(.2,.3,'dark',4),(.21,.3,'dark',3),(.95,.95,'dark',1)]]
        c.json(existing=existing,additional=additional,width=160,height=128,maximum=40,output=soft.merge(existing,additional,160,128,40))
    with Case('dust','cancel') as c:
        records=[]
        for n in [1,2,5,15,35,100,250]:
            token=CancelAfter(n);events=[]
            try:dust.detect(data['grain'],dust.Options(detailed=True,soft=True,scratches=True),cancel=token,progress=lambda a,b:events.append([a,b]),tile_size=48);cancelled=False
            except dust.Cancelled:cancelled=True
            records.append(dict(after=n,calls=token.calls,cancelled=cancelled,progress=events))
        c.json(cases=records)


def validation():
    records=[]
    cases=[('options',{'sensitivity':0}),('options',{'sensitivity':101}),('options',{'minimum':1}),('options',{'minimum':41}),('options',{'maximum':161}),('options',{'polarity':'invalid'}),('options',{'detailed':1}),('options',{'soft':None}),('options',{'scratch_width':2.0}),('options',{'scratch_width':41}),('spot',{}),('spot',{'x':2,'y':0,'rx':.1,'ry':.1}),('spot',{'x':0,'y':0,'rx':0,'ry':.1}),('spot',{'x':0,'y':0,'rx':.1,'ry':.1,'parts':[]}),('spot',{'x':0,'y':0,'rx':.1,'ry':.1,'parts':[{'x':0,'y':0,'rx':.1,'ry':.1,'extra':1}]}),('spot',{'x':0,'y':0,'rx':.1,'ry':.1,'scale':24}),('manual',{'points':[],'diameter':3}),('manual',{'points':[[1.1,0]],'diameter':3}),('manual',{'points':[[0,0]],'diameter':0}),('visualize',0),('visualize',101),('detect_tile',31),('detect_limit',0),('repair','unknown')]
    for function,args in cases:
        try:
            if function=='options':dust.Options(**args).validate()
            if function=='spot':shapes.ellipses(args,(10,10))
            if function=='manual':shapes.manual(args['points'],(10,10),args['diameter'])
            if function=='visualize':dust.visualize(np.zeros((2,2),np.float32),args)
            if function=='detect_tile':dust.detect(np.zeros((2,2),np.float32),tile_size=args)
            if function=='detect_limit':dust.detect(np.zeros((2,2),np.float32),limit=args)
            if function=='repair':dust.repair(np.zeros((2,2,3),np.float32),[],method=args)
            rejected=False
        except (ValueError,TypeError,KeyError):rejected=True
        records.append(dict(function=function,args=args,rejected=rejected))
    packed=[b'',b'bad',zlib.compress(b'hello')[:-1],zlib.compress(b'hello')+b'x',zlib.compress(b'a')+zlib.compress(b'b'),zlib.compress(b'\xff'),zlib.compress(b'a'*(store.MAX_ENTRY_BYTES+1))]
    with Case('dust','validation') as c:
        for i,p in enumerate(packed):
            c.array(f'bad_payload_{i}',np.frombuffer(p,np.uint8))
            try:store.unpack(p);raise AssertionError('oracle accepted corrupt payload')
            except (ValueError,UnicodeError,zlib.error):pass
        c.json(cases=records,bad_payloads=len(packed))


def pixels(data,detected):
    with Case('dust','visualize') as c:
        records=[]
        for name in ['smooth','grain','pattern','constant','standard_wide']:
            for strength in [1,50,100]:
                key=f'{name}_{strength}';c.array(key,dust.visualize(data[name],strength));records.append(dict(name=name,strength=strength,array=key))
        c.json(cases=records)
    rng=np.random.default_rng(32189)
    with Case('dust','repair') as c:
        h,w=64,80;yy,xx=np.mgrid[:h,:w].astype(np.float32)
        a=np.stack([.2+xx*.002,.3+yy*.002,.6-xx*.001],-1)+rng.normal(0,.018,(h,w,3)).astype(np.float32)
        c.array('input',a)
        groups=[[],[dict(x=.35,y=.45,rx=.04,ry=.05)], [dict(x=x,y=y,rx=.035,ry=.045) for x,y in [(.2,.3),(.4,.3),(.43,.32),(.7,.7),(.8,.75),(.6,.3)]], [shapes.manual([[.15,.3],[.4,.55],[.8,.4]],(h,w),3)], [dict(x=0,y=0,rx=.06,ry=.06),dict(x=1,y=1,rx=.06,ry=.06)], [dict(x=.5,y=.5,rx=.5,ry=.5,scale=250)]]
        cases=[]
        for i,spots in enumerate(groups):
            for method in ['smooth','texture']:
                source=a.copy();out=dust.repair(source,spots,method=method,copy=True);assert np.array_equal(source,a)
                in_place=a.copy();assert dust.repair(in_place,spots,method=method,copy=False) is in_place;assert np.array_equal(in_place,out)
                key=f'output_{i}_{method}';c.array(key,out);cases.append(dict(spots=spots,method=method,array=key,checksum=checksum(out)))
        c.json(cases=cases)
    # Store every selected pixel, plus an exact checksum and an outside-mask identity assertion.
    # No duplicate RGB input or full repaired frame is needed for the large seam scan.
    with Case('dust','repair_detected') as c:
        cases=[]
        for name,gray in data.items():
            source=np.repeat(gray[...,None],3,axis=2)
            spots=next(v['result']['spots'] for v in detected if v['name']==name)
            if name=='hairs':spots=next(v['result']['spots'] for v in detected if v['name']==name and v['options']['scratches'] and v['options']['scratch_width']==12)
            mask=np.zeros(gray.shape,np.uint8)
            for spot in spots:shapes.paint(mask,shapes.ellipses(spot,gray.shape))
            for method in ['smooth','texture']:
                out=dust.repair(source,spots,method=method);key=f'{name}_{method}';c.array(key,out[mask>0]);assert np.array_equal(out[mask==0],source[mask==0]);cases.append(dict(name=name,spots=spots,method=method,array=key,checksum=checksum(out)))
        c.json(cases=cases)
    with Case('dust','texture') as c:
        cases=[]
        for i,(h,w,kind) in enumerate([(8,9,'tiny'),(40,48,'none'),(40,48,'full'),(64,80,'selected'),(64,80,'border')]):
            a=rng.random((h,w,3),dtype=np.float32)*.2+.4;smooth=cv2.GaussianBlur(a,(13,13),1.5);mask=np.zeros((h,w),np.uint8)
            if kind=='full':mask[:]=255
            if kind in ('tiny','selected'):mask[h//2-2:h//2+2,w//2-2:w//2+2]=255
            if kind=='border':mask[:3,:5]=255;mask[-3:,-5:]=255
            c.array(f'original_{i}',a);c.array(f'smooth_{i}',smooth);c.array(f'mask_{i}',mask);stats={};out=texture.restore(a,smooth.copy(),mask,stats=stats);c.array(f'output_{i}',out)
            active=np.maximum.reduceat(np.maximum.reduceat(mask,np.arange(0,h,8),axis=0),np.arange(0,w,8),axis=1);blocks=list(zip(*np.nonzero(active)));pending,transferred,fallback=texture._prepare(a,smooth,mask,blocks,None)
            cases.append(dict(stats=stats,blocks=blocks,transferred=transferred,fallback=fallback,checksum=checksum(out)))
        c.json(cases=cases)


def storage():
    with Case('dust','store') as c:
        values=[{'state':'scanning','record':{'id':1,'path':'generated/사진.jpg','settings':{'exposure':1.5}},'fingerprint':None},{'state':'reviewed','nested':[1,True,None,{'value':'Straße 𐀀'}]},[],{'large':'x'*4096}]
        for i,v in enumerate(values):
            if isinstance(v,dict) and v.get('state')=='scanning':v['state']='pending'
            raw=json.dumps(v,ensure_ascii=False).encode('utf-8');payload=zlib.compress(raw,3);c.array(f'payload_{i}',np.frombuffer(payload,np.uint8))
        snapshots=[({'folder':None,'scopes':{'selection':[3,1,3,2],'folder':[]}},'selection'),({'folder':None,'scopes':{'selection':[],'folder':[9,2,9]}},'folder'),({'folder':r'Z:\Dust generated\missing\Straße','scopes':{'folder':[]}},'folder')]
        c.json(values=values,targets=[dict(snapshot=s,scope=scope,key=store.target_key(s,scope)) for s,scope in snapshots])
    from luma.catalog import Catalog
    folder=ROOT/'target'/'dust-generated-catalog';folder.mkdir(parents=True,exist_ok=True)
    # Fresh deterministic catalog, deleting only this generator's known files.
    for name in ['catalog.sqlite','catalog.sqlite-wal','catalog.sqlite-shm']:
        (folder/name).unlink(missing_ok=True)
    catalog=Catalog(folder)
    paths=[r'Z:\Dust generated\Scan.JPG',r'Z:\Dust generated\other.tif',r'Z:\Dust generated\nested\child.jpg',r'Z:\Dust generated\movie.MOV',r'Y:\elsewhere\image.png']
    class Clock(datetime):
        @classmethod
        def now(cls,tz=None):
            value=datetime(2026,11,13,1,2,3,tzinfo=timezone.utc)
            return value if tz else value.replace(tzinfo=None)
    with patch('luma.catalog.datetime',Clock):
        for i,path in enumerate(paths):catalog.add(path,{'exposure':i*.25},{},resolved=True)
    # Preview tokens are irrelevant to dust capture; stabilize only this generated fixture.
    catalog.db.execute("UPDATE preview_state SET token=printf('%032x',photo_id)")
    scopes=[]
    for current,selection,parent in [(1,[2,1,2,999],None),(999,[],None),(None,[4,3],r'Z:\Dust generated'),(1,[],r'Y:\elsewhere')]:scopes.append(dict(current=current,selection=selection,folder=parent,output=batch.capture_scope_ids(catalog,current,selection,parent)))
    records=[batch.capture_record(catalog,i) for i in [1,2,3,5]]
    catalog.db.commit()
    with Case('dust','batch') as c:
        dest=c.folder/'catalog.sqlite';dest.unlink(missing_ok=True)
        import sqlite3
        with sqlite3.connect(dest) as db:catalog.db.backup(db)
        c.json(scopes=scopes,records=records)
    catalog.close()


def main():
    if '--check-rust' in sys.argv:return check_rust()
    data=scenes()
    with Case('dust','scenes') as c:
        for name,a in data.items():c.array(name,a)
        c.json(names=list(data))
    detected=detection(data);helpers(data);pixels(data,detected);storage();validation()
    size=sum(p.stat().st_size for p in (GOLDEN/'dust').rglob('*') if p.is_file())
    assert size<10_000_000,size
    print('dust goldens bytes:',size,flush=True)


if __name__=='__main__':main()
