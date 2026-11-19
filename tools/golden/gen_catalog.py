"""Generated-only fixtures; run with -B to keep the Python oracle read-only.

--check-rust DIRECTORY validates the Rust test round-trip artifact with the oracle.
"""
import sys
sys.dont_write_bytecode = True
from common import ROOT, ORACLE, GOLDEN, Case, write_json
import ast
import json
import base64
import hashlib
import os
import shutil
import sqlite3
import tempfile
import threading
from pathlib import Path, PureWindowsPath, PurePosixPath
from datetime import datetime, timezone
from unittest.mock import patch
from luma.engine import defaults
from luma.catalog import Catalog
from luma import catalog as catalog_module, folders, library_query, workflow_data, preview_store, command_batch

STAMP = '2026-11-13T01:02:03.456789+00:00'
TOKEN = '0123456789abcdef0123456789abcdef'


class Clock(datetime):
    @classmethod
    def now(cls, tz=None):
        value = datetime.fromisoformat(STAMP)
        return value if tz else value.replace(tzinfo=None)


def dump(db):
    result = {}
    tables = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    for table in tables:
        columns = [r[1] for r in db.execute('PRAGMA table_info(' + table + ')')]
        rows = []
        for row in db.execute('SELECT * FROM ' + table + ' ORDER BY ' + ','.join('"' + c + '"' for c in columns)):
            item = dict(zip(columns, row))
            if table == 'preview_state':
                item['token'] = TOKEN
            item = {k: {'blob': bytes(v).hex()} if isinstance(v, bytes) else v for k, v in item.items()}
            rows.append(item)
        result[table] = rows
    return result


def readers(c):
    ids = [p['id'] for p in c.photos()]
    prefs = [r[0] for r in c.db.execute('SELECT key FROM preferences ORDER BY key')]
    return dict(photos=c.photos(), paths=c.paths(), photo={str(i): c.photo(i) for i in ids + [99999]},
                histories={str(i): c.histories(i) for i in ids}, snapshots={str(i): c.snapshots(i) for i in ids},
                presets=c.presets(), folder_roots=c.folder_roots(), removed_folders=c.removed_folders(),
                collections=c.collections(), collection_ids={str(r['id']): sorted(c.collection_ids(r['id'])) for r in c.collections()},
                preferences={k: c.preference(k) for k in prefs}, missing_preference=c.preference('missing', {'fallback': True}))


def operation(c, op):
    name, a = op['method'], op.get('args', [])
    if name == 'update': return c.update(a[0], **a[1])
    if name == 'add': return c.add(a[0], a[1], a[2], resolved=True)
    if name == 'register_folder': return c.register_folder(a[0], resolved=True)
    if name == 'backup': return c.backup(c.directory / a[0]).name
    if name == 'readers': return readers(c)
    if name == 'query': return library_query.query(c.directory / 'catalog.sqlite', a[0], threading.Event())
    if name == 'read_page': return library_query.read_page(c.directory / 'catalog.sqlite', a[0])
    if name == 'repeat_edit':
        for i in range(a[1]): c.edit(a[0], {'exposure': i / 100}, 'history-cap')
        return len(c.histories(a[0]))
    if name == 'sql':
        c.db.execute(a[0], a[1]); c.db.commit(); return None
    if name == 'save_adobe_preset': return c.save_adobe_preset(*a)
    if name.startswith('workflow.'):
        f = getattr(workflow_data, name.split('.')[1])
        return f(*a) if name in ('workflow.arrange_stacks','workflow.edit_signature') else f(c, *a)
    if name == 'text_hash': return command_batch.text_hash(*a)
    result = getattr(c, name)(*a)
    return sorted(result) if isinstance(result, set) else result


def scripts(profile):
    p1 = r'Z:\Grainy Generated\사진\Straße 한글.jpg'
    p2 = r'Z:\Grainy Generated\사진\nested_%~\Ｂeta.NEF'
    p3 = 'Y:\\Other\\' + 'long-' * 50 + 'é.png'
    adobe = b'<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"><rdf:Description crs:Exposure2012="1.25" crs:HasCrop="False"/></rdf:RDF>'
    adobe_record = {'version': 1, 'format': 'xmp', 'data': base64.b64encode(adobe).decode(), 'sha256': hashlib.sha256(adobe).hexdigest()}
    ops=[]
    def add(method,*args,checkpoint=False): ops.append(dict(method=method,args=list(args),checkpoint=checkpoint))
    add('readers')
    add('add',p1,{}, {'width':4000,'height':3000,'DateTimeOriginal':'2025:12:31 23:59:59','camera':'Straße Camera','iso':'400'})
    add('add',p1.upper(),{'exposure':5},{})
    add('add',p2,{'exposure':2,'sharpen':40},{'width':2000,'height':3000,'date':'2026-11-12','iso':1600})
    add('add',p3,{'noise_luma':20,'unknown':3},{'width':200,'height':200})
    add('add',r'Z:\Grainy Generated\사진\movie.MOV',{}, {})
    add('update',1,{'rating':5,'flag':1,'label':'빨강','keywords':'City, Straße, 한글','ignored':4})
    add('update',2,{'rating':4,'flag':-1,'label':'custom','keywords':'forest, snow'})
    add('update_many',[1,3,999],'rating',3)
    add('update_many',[1,3],'label','파랑')
    add('update_many',[1],'flag',1)
    add('update_many',[3],'stack_id',44)
    add('set_user_metadata',1,{'caption':'눈 ☃️','latitude':'37.5','longitude':127,'DateTimeOriginal':'2026:11:13 00:00:00'})
    add('edit',1,{},'no-op')
    add('edit',1,{'exposure':0.0,'negative':0},'numeric no-op')
    add('set_settings',1,{'exposure':False,'negative':0.0})
    add('edit',1,{'exposure':1.5,'panels_off':['detail','bogus']},'보정 α')
    add('edit',999,{},'missing')
    add('set_settings',2,{'exposure':-5,'dcp_profile':profile})
    add('set_settings',2,{'exposure':-5,'dcp_profile':profile})
    add('set_settings',999,{})
    add('snapshot',2,'DCP before')
    add('virtual_copy',2)
    add('virtual_copy',5)
    add('stack',[1,5,6])
    add('stack',[])
    add('snapshot',1,'이전')
    add('save_preset','한국 preset',{'exposure':3,'crop':[.1,.2,.8,.9],'rotation':90,'straighten':4,'flip':True,'dcp_profile':profile})
    add('save_preset','한국 preset',{'exposure':-3,'dcp_profile':profile})
    add('save_preset','delete me',{})
    add('delete_preset','delete me')
    add('save_adobe_preset','Adobe',{'_adobe_preset':adobe_record})
    add('save_preference','undo:2',{'undo':[{'dcp_profile':profile,'exposure':1}],'redo':[],'extra':'保留'})
    add('save_preference','raw_defaults_v1',{'profiles':[{'dcp_profile':profile}]})
    add('save_preference','custom',{'value':[None,True,1e-7,1e16,-0.0,'☃️한글']})
    add('save_preference','undo:command_batch',{'v':2,'state':'applied','id':'batch-fixture','records':[{'id':1,'path':p1,'before':{'exposure':0},'after_set':{'exposure':1},'after_drop':[],'undo_keys':['undo','redo'],'undo_head':[],'redo_before':[],'undo_extra':{},'h_before':'a'*64,'h_after':'b'*64}]})
    add('add_collection','Manual',None,None)
    add('add_collection','Smart',{'any':[{'rating_min':4},{'keywords':'strasse'}]},None)
    add('add_collection','Child',None,1)
    add('add_collection','Adobe smart',{'_adobe_smart':{'combine':'union','children':[{'criteria':'pick','operation':'==','value':1},{'criteria':'fileFormat','operation':'==','value':'RAW'}]}},None)
    add('collection_add',1,[1,2,2,999])
    add('collection_ids',999)
    add('workflow.stack_order',6,0)
    add('workflow.stack_order',5,None)
    add('workflow.stack_members',3)
    add('workflow.toggle_quick',[1,2])
    add('workflow.toggle_quick',[1,2])
    add('workflow.toggle_quick',[])
    add('workflow.save_metadata_preset','  Location  ',{'title':'서울','latitude':37,'longitude':127,'DateTimeOriginal':'2026:11:12 23:59:59','keywords':'one, two'})
    add('workflow.metadata_presets')
    add('workflow.apply_metadata_preset',[1,5],{'creator':'작가','keywords':'Street, Straße'})
    add('workflow.add_keyword',[1,5],' STRASSE ')
    add('workflow.add_keyword',[1,5],'여행|서울')
    add('workflow.edit_signature',{'exposure':1e-7,'dcp_profile':profile})
    add('text_hash','{"undo": [], "redo": []}')
    add('workflow.record_exports',[{'photo_id':1,'path':'generated.jpg','edit_signature':'older'}])
    add('workflow.export_history',[1,2])
    for options in [ {}, {'search':'strasse','sort':'name'}, {'sort':'capture','descending':True}, {'mode':'picked'}, {'mode':'rejected'}, {'mode':'rated'}, {'folder':r'Z:\Grainy Generated\사진','recursive':False}, {'folder':r'Z:\Grainy Generated\사진','recursive':True}, {'folder':r'Z:\Grainy Generated\사진\nested_%~'}, {'collection':1}, {'collection':2}, {'collection':4}, {'collection':999}, {'sort':'rating','descending':True,'collapsed':True,'stack_orders':{'1':[6,5]}}, {'rules':{'all':[{'orientation':'portrait'},{'virtual':True}]}}, {'rules':{'date_from':'2026-11-12','date_to':'2026-11-13'}}, {'rules':{'missing':True}}, {'rules':{'_adobe_smart':{'criteria':'captureTime','operation':'today'}}} ]:
        add('query',options)
    add('read_page',[1,5,999])
    add('read_page',[])
    add('readers',checkpoint=True)
    add('repeat_edit',3,83)
    add('histories',3)
    add('save_preference','removed_folders',[r'Z:\Grainy Generated\사진\nested_%~',r'Z:\Other'])
    add('register_folder',r'Z:\Grainy Generated\사진\nested_%~')
    add('save_preference','watch_folders',[r'Z:\Grainy Generated\사진\nested_%~',r'Z:\Other'])
    add('folder_photo_ids',r'Z:\Grainy Generated\사진\nested_%~')
    add('backup','backups/manual.sqlite')
    add('remove_collection',1)
    add('remove_folder',r'Z:\Grainy Generated\사진\nested_%~',checkpoint=True)
    add('restore_folder',r'Z:\Grainy Generated\사진')
    add('remove_photos',[])
    add('remove_photos',[3,999],checkpoint=True)
    add('readers',checkpoint=True)
    return ops


def generate_operations():
    profile_bytes=b'Generated DCP storage test, not an image profile\x00\xff'
    profile={'name':'Generated 한글','sha256':hashlib.sha256(profile_bytes).hexdigest(),'data':base64.b64encode(profile_bytes).decode()}
    with tempfile.TemporaryDirectory(prefix='grainy-catalog-oracle-') as temporary, Case('catalog','operations') as case:
        c=Catalog(temporary); c.close()
        shutil.copy2(Path(temporary)/'catalog.sqlite',case.folder/'start.sqlite')
        c=Catalog(temporary)
        ops=scripts(profile)
        previous=dump(c.db)
        for op in ops:
            op['expected']=operation(c,op)
            current=dump(c.db)
            op['delta']={k:v for k,v in current.items() if v!=previous.get(k)}
            previous=current
            if op['checkpoint']: op['tables']=dump(c.db)
            if op['method']=='save_adobe_preset':
                from luma.adobe_preset import translate
                op['translated']=translate(op['args'][1]['_adobe_preset'])
        case.json(steps=ops,final_tables=dump(c.db),final_readers=readers(c),stamp=STAMP,token=TOKEN)
        c.close();shutil.copy2(Path(temporary)/'catalog.sqlite',case.folder/'oracle.sqlite')
    # Preserve operation argument order: Python JSON storage hashes are order-sensitive.
    (case.folder/'case.json').write_text(json.dumps(case.meta,ensure_ascii=False,indent=1)+'\n',encoding='utf-8',newline='\n')


def generate_migrations():
    for stage in ('v1','v2','v3','preview','profiles','current'):
        with tempfile.TemporaryDirectory(prefix='grainy-catalog-migrate-') as temporary, Case('catalog','migration_'+stage) as case:
            c=Catalog(temporary)
            c.add(r'Z:\Generated\가\old.jpg',resolved=True)
            c.add(r'Z:\Generated\나\older.jpg',resolved=True)
            c.db.execute('UPDATE photos SET edits=? WHERE id=1',(json.dumps({'exposure':2,'sharpen':20}),))
            c.db.execute('UPDATE photos SET edits=? WHERE id=2',(json.dumps({'noise_luma':10,'tone_version':2,'shadows':20}),))
            c.db.execute('DELETE FROM folder_roots');c.db.commit()
            if stage=='current':
                import zlib
                c.db.execute('INSERT INTO dust_sessions VALUES(?,?,?,?,?,?,?,?,?,?)',('generated-session',1,'target','selection','{}','{}',STAMP,STAMP,0,1))
                c.db.execute('INSERT INTO dust_entries VALUES(?,?,?,?,?,?)',('generated-session',0,1,r'Z:\Generated\가\old.jpg','pending',zlib.compress(b'{"generated": true}')))
                c.db.commit()
            if stage!='current':
                c.db.executescript('DROP TABLE dust_sessions; DROP TABLE dust_entries;')
            if stage not in ('profiles','current'):c.db.execute('DROP TABLE profile_assets')
            if stage in ('v1','v2','v3'):
                c.db.executescript('DROP TRIGGER preview_added; DROP TRIGGER preview_changed; DROP TRIGGER preview_removed; DROP TABLE preview_state;')
            if stage in ('v1','v2'):
                c.db.executescript('DROP TRIGGER browser_added; DROP TRIGGER browser_changed; DROP TRIGGER browser_removed; DROP TABLE browser_index; DROP TABLE browser_dirty; DROP INDEX photos_rating; DROP INDEX photos_flag;')
            if stage=='v1':
                c.db.executescript('''ALTER TABLE photos RENAME TO previous;
                  CREATE TABLE photos(id INTEGER PRIMARY KEY,path TEXT UNIQUE NOT NULL,name TEXT NOT NULL,rating INTEGER NOT NULL DEFAULT 0,flag INTEGER NOT NULL DEFAULT 0,keywords TEXT NOT NULL DEFAULT '',edits TEXT NOT NULL,metadata TEXT NOT NULL DEFAULT '{}',imported TEXT NOT NULL);
                  INSERT INTO photos SELECT id,path,name,rating,flag,keywords,edits,metadata,imported FROM previous;
                  DROP TABLE previous;''')
            c.db.commit();c.close();shutil.copy2(Path(temporary)/'catalog.sqlite',case.folder/'start.sqlite')
            c=Catalog(temporary);case.json(tables=dump(c.db),readers=readers(c),schema=[list(r) for r in c.db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")],backups=sorted(p.name for p in (Path(temporary)/'backups').glob('*.sqlite')));c.close()


def generate_folders():
    import ntpath,posixpath
    from types import SimpleNamespace
    with Case('catalog','folders') as case:
        scenarios=[]
        for platform,path_module,path_class,mac in [('windows',ntpath,PureWindowsPath,False),('mac',posixpath,PurePosixPath,True),('posix',posixpath,PurePosixPath,False)]:
            paths=[r'C:\Photos\..\É\a.jpg',r'c:/É//nested/../b.jpg',r'\\server\share\한글\a.jpg',r'D:\Else\a.jpg','C:\\',r'C:relative\x',r'.\foo\..\bar','.'] if platform=='windows' else ['/Photos/../É/a.jpg','/É//nested/../b.jpg','/한글/A.jpg','/한글/a.jpg','/','./foo/../bar','.','//server/share/file','///server/share/file']
            with patch.object(folders,'os',SimpleNamespace(path=path_module)),patch.object(folders,'Path',path_class),patch.object(folders,'MAC',mac):
                items=[{'path':p,'key':folders.path_key(p),'stored':folders.stored_path(p)} for p in paths]
                photos=[{'path':str(path_class(paths[0]).parent/'one.jpg'),'count':2},{'path':str(path_class(paths[0]).parent/'nested'/'two.jpg')},{'path':str(path_class(paths[4])/'three.jpg')}]
                root=str(path_class(paths[0]).parent)
                registered=[root,str(path_class(root)/'nested')]
                relations=[]
                for a in paths:
                    for b in paths:
                        relations.append({'a':a,'b':b,'in':folders.in_folder(a,b),'direct':folders.in_folder(a,b,False),'contains':folders.contains_folder(a,b),'below':str(folders.below(a,b)) if folders.contains_folder(b,a) else None})
                scenarios.append(dict(platform=platform,items=items,relations=relations,photos=photos,roots=registered,nodes=folders.folder_nodes(photos,registered),left_out=[dict(path=root,registered=[],removed=[root],expected=folders.left_out(root,[],[root])),dict(path=root,registered=[root],removed=[root],expected=folders.left_out(root,[root],[root]))]))
        case.json(scenarios=scenarios)


def generate_preview():
    import numpy as np
    from PIL import Image
    with Case('catalog','preview') as case:
        image=np.arange(12*16*3,dtype=np.uint8).reshape(12,16,3)
        Image.fromarray(image).save(case.folder/'packet.jpg',quality=88)
        for name,native in [('half',False),('raw',True)]:
            pixels=np.linspace(-.1,2,4*6*3,dtype=np.float32).reshape(4,6,3)
            info={'width':600,'height':400,'working_space':'sRGB','raw_native':native,'label':'생성한 픽셀'}
            if native:info['raw_info']={'white_level':16383,'camera':'synthetic'}
            extra={'legacy_pixels':(pixels*.5).astype(np.float16)} if native else {}
            np.savez_compressed(case.folder/(name+'.npz'),pixels=pixels.astype(np.float32 if native else np.float16),info=json.dumps(info),**extra)
            case.array(name+'_pixels',pixels if native else pixels.astype(np.float16).astype(np.float32))
            if native:case.array(name+'_legacy',(pixels*.5).astype(np.float16).astype(np.float32))
        payload=(case.folder/'packet.jpg').read_bytes()
        state={'id':1,'path':r'Z:\Generated\preview.jpg','token':TOKEN,'source':{'path':r'Z:\Generated\preview.jpg','size':12,'mtime_ns':123},'offline':None,'denied':False}
        stamp={k:state[k] for k in ('token','path','source','offline')}
        stamp.update(version=4,color_space='sRGB',sha256=hashlib.sha256(payload).hexdigest())
        scenarios=[]
        variants=[('valid',{}, {},'jpeg'),('missing-source',{'source':None},{},'jpeg'),('stale-token',{}, {'token':'stale'},'jpeg'),('version',{}, {'version':3},'jpeg'),('hash',{}, {'sha256':'wrong'},'jpeg'),('path',{}, {'path':'wrong'},'jpeg'),('source-changed',{}, {'source':None},'jpeg'),('ProPhoto',{}, {'color_space':'ProPhoto RGB'},'jpeg'),('bad-space',{}, {'color_space':'unknown'},'jpeg'),('null-space',{}, {'color_space':None},'jpeg'),('numeric-version',{}, {'version':4.0},'jpeg'),('missing-packet',{}, {},'none'),('corrupt',{}, {},'corrupt'),('missing-old',{'source':None},{'token':'old'},'jpeg'),('missing-all',{'source':None},{},'none'),('denied-valid',{'source':None,'denied':True},{},'jpeg'),('denied-old',{'source':None,'denied':True},{'token':'old'},'jpeg'),('denied-empty',{'source':None,'denied':True},{},'none'),('offline-valid',{'source':None,'offline':{'path':'offline','size':3,'mtime_ns':4}},{'source':None,'offline':{'path':'offline','size':3,'mtime_ns':4}},'jpeg'),('offline-changed',{'source':None,'offline':{'path':'offline','size':3,'mtime_ns':5}},{'source':None,'offline':{'path':'offline','size':3,'mtime_ns':4}},'jpeg'),('offline-from-online',{'source':None,'offline':{'path':'offline','size':3,'mtime_ns':5}},{},'jpeg'),('malformed-stamp',{},None,'jpeg'),('removed',None,{},'jpeg')]
        with tempfile.TemporaryDirectory(prefix='grainy-preview-') as temporary:
            directory=Path(temporary);(directory/'thumbnails').mkdir()
            for name,change,meta,kind in variants:
                current=None if change is None else {**state,**change}
                packet=None if meta is None else {**stamp,**meta}
                jpeg,stamp_file=preview_store.files(directory,1)
                jpeg.unlink(missing_ok=True)
                if kind!='none':jpeg.write_bytes(payload if kind=='jpeg' else b'invalid jpeg')
                stamp_file.write_text(json.dumps(packet),encoding='utf-8')
                with patch.object(preview_store,'snapshot',return_value=current):result=preview_store.inspect(directory,1)
                result['image']=result['image'] is not None
                scenarios.append(dict(name=name,current=current,stamp=packet,kind=kind,expected=result))
        case.json(version=preview_store.VERSION,scenarios=scenarios)
        loads=[]
        from luma.engine import normalized
        from luma.rawcolor import CameraSource
        with tempfile.TemporaryDirectory(prefix='grainy-offline-') as temporary:
            directory=Path(temporary);(directory/'previews').mkdir()
            for name,options in [('half',{}),('raw',{}),('raw',{'raw_mode':'as_shot'}),('raw',{'negative':True}),('half',{'raw_mode':'as_shot'}),('half',{'working_space':'ProPhoto'})]:
                shutil.copy2(case.folder/(name+'.npz'),directory/'previews/1.npz')
                value=normalized(options)
                # The catalog owns packet selection and metadata; the color crate owns conversion.
                def convert(pixels,source,destination):return pixels*2
                with patch('luma.colorio.profile',side_effect=lambda name:name),patch('luma.colorio.convert',side_effect=convert):
                    try:
                        pixels,info=preview_store.load_offline(directory,1,value)
                    except ValueError:
                        loads.append(dict(name=name,settings=value,error=True));continue
                info['offline_fingerprint']='<fingerprint>'
                loads.append(dict(name=name,settings=value,error=False,pixels=pixels.tolist(),info=info,native=isinstance(pixels,CameraSource)))
        case.json(loads=loads)


def generate_predicates():
    from luma.library import matches
    with Case('catalog','predicates') as case:
        photo=dict(id=3,path=r'Z:\Generated\Straße portrait.NEF',name='Renamed',keywords='street,한글,red apple',rating=4,flag=-1,label='빨강',virtual_source=1,stack_id=3,imported=STAMP,
            metadata=json.dumps(dict(width=20,height=40,DateTimeOriginal='2026:11:12 23:00:00',iso='400',camera='Straße',serial='SN123',lens='Prime',title='Red apple',caption='  ',country='Korea',state='Seoul',city='Seoul',location='Park',creator='Artist',job='Trip',latitude=37,longitude=127)),
            extras=json.dumps({'_copy_name':'Copy 1','_last_edit_time':STAMP}))
        rules=[{}, {'any':[{'rating':1},{'flag':-1}]},{'all':[{'rating_min':4},{'rating_max':4}]},{'all':[{'rating':4},{'flag':1}]},{'virtual':False},{'virtual':True},{'date_from':'2027-01-01'},{'date_to':'2020-01-01'},{'orientation':'landscape'},{'orientation':'square'},{'orientation':'portrait'},{'rating_min':'bad'},{'label':'빨강'},{'folder':'GENERATED'},{'extension':'nef'},{'caption':''}]
        rules.extend([{'rating':4.0},{'flag':-1.0},{'rating':True}])
        for field in ('rating','pick','isoSpeedRating'):
            for op in ('==','!=','>','<','>=','<=','in'):
                rules.append({'_adobe_smart':dict(criteria=field,operation=op,value=4,value2=400)})
        for field in ('filename','copyname','folder','labelText','title','caption','keywords','camera','cameraSN','lens','country','state','city','location','creator','jobIdentifier'):
            for op in ('any','all','words','noneOf','beginsWith','endsWith','empty','notEmpty','==','!='):
                rules.append({'_adobe_smart':dict(criteria=field,operation=op,value='red' if field=='labelText' else 'red apple' if field in ('keywords','title') else 'strasse')})
        for field in ('captureTime','touchTime'):
            for op in ('==','!=','>','<','in','today','yesterday','thisWeek','thisMonth','thisYear'):
                rules.append({'_adobe_smart':dict(criteria=field,operation=op,value='2026-11-12',value2='2026-11-13')})
            for op in ('inLast','notInLast'):
                for unit in ('hours','days','weeks','months','years'):
                    for n in (0,1,10000):rules.append({'_adobe_smart':dict(criteria=field,operation=op,value=n,value_units=unit)})
        for value in (1,2,3,4,5,'none','custom'):
            for op in ('==','!='):rules.append({'_adobe_smart':dict(criteria='labelColor',operation=op,value=value)})
        for fmt in ('DNG','RAW','JPG','TIFF','PNG','PSD','VIDEO','PSB','AVIF','JXL'):
            for op in ('==','!='):rules.append({'_adobe_smart':dict(criteria='fileFormat',operation=op,value=fmt)})
        for op in ('isTrue','isFalse'):rules.append({'_adobe_smart':dict(criteria='hasGPSData',operation=op)})
        for mode in ('union','intersect','exclude'):rules.append({'_adobe_smart':{'combine':mode,'children':[{'criteria':'rating','operation':'==','value':4},{'criteria':'filename','operation':'words','value':'portrait'}]}})
        # Use a naive edited time to keep relative-date fixtures independent of the host time zone.
        photo['extras']=json.dumps({'_copy_name':'Copy 1','_last_edit_time':'2026-11-13T01:02:03.456789'})
        case.json(photo=photo,now='2026-11-13T01:02:03.456789',rules=[dict(rule=r,expected=matches(photo,r,now=Clock.now())) for r in rules])


def generate_batches():
    for compact in (False,True):
        with tempfile.TemporaryDirectory(prefix='grainy-batch-') as temporary,Case('catalog','batch_v2' if compact else 'batch_v1') as case:
            c=Catalog(temporary);ident=c.add(r'Z:\Generated\batch.jpg',resolved=True)
            c.save_preference('undo:'+str(ident),{'undo':[{'exposure':i/100,'extra':i} for i in range(81)],'redo':[{'exposure':-1}],'custom':'preserve'})
            snap=command_batch.capture(c,ident,[ident]);records=command_batch.targets(snap,'selection')
            command_batch.apply(c,records,[{'exposure':1.5}],compact=compact,switch=False,meta={'id':'oracle-batch'})
            case.json(readers=readers(c),tables=dump(c.db));c.close();shutil.copy2(Path(temporary)/'catalog.sqlite',case.folder/'start.sqlite')
        (case.folder/'case.json').write_text(json.dumps(case.meta,ensure_ascii=False,indent=1)+'\n',encoding='utf-8',newline='\n')


def generate_legacy_profile():
    with tempfile.TemporaryDirectory(prefix='grainy-legacy-profile-') as temporary,Case('catalog','legacy_profile') as case:
        c=Catalog(temporary);ident=c.add(r'Z:\Generated\legacy.jpg',resolved=True)
        c.db.execute('UPDATE photos SET edits=? WHERE id=?',(json.dumps({'dcp_profile':{'sha256':'invalid','data':'broken'},'exposure':2}),ident));c.db.commit();c.close()
        shutil.copy2(Path(temporary)/'catalog.sqlite',case.folder/'start.sqlite')
        c=Catalog(temporary);c.edit(ident,{'exposure':3},'repair legacy profile');case.json(tables=dump(c.db),readers=readers(c));c.close()


def check_rust(directory):
    directory=Path(directory).resolve()
    if not directory.is_relative_to(ROOT/'target'):raise ValueError('roundtrip input must be under this worktree target/')
    expected=json.loads((GOLDEN/'catalog/operations/case.json').read_text(encoding='utf-8'))
    before=(directory/'catalog.sqlite').read_bytes()
    with Catalog.open_reader(directory) as c:
        assert readers(c)==expected['final_readers'], 'oracle reader roundtrip differs'
        assert dump(c.db)==expected['final_tables'], 'oracle table roundtrip differs'
    assert before==(directory/'catalog.sqlite').read_bytes()
    for name in ('half','raw'):
        import numpy as np
        with np.load(directory/(name+'.npz'),allow_pickle=False) as a,np.load(GOLDEN/'catalog/preview'/(name+'.npz'),allow_pickle=False) as b:
            assert set(a.files)==set(b.files)
            for key in a.files:assert np.array_equal(a[key],b[key]),(name,key)
    print('PASS: Python Catalog read every Rust-written table and public reader; float16/float32 NPZ packets round-trip exactly.')


def bootstrap():
    assets = ROOT / 'crates/grainy-catalog/assets'
    assets.mkdir(parents=True, exist_ok=True)
    write_json(assets / 'default_order.json', list(defaults()))
    blocks = {}
    for module in ('catalog', 'preview_store', 'dust_store', 'profile_store'):
        tree = ast.parse((ORACLE / 'luma' / (module + '.py')).read_text(encoding='utf-8'))
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                for item in ast.walk(node):
                    if isinstance(item, ast.Constant) and isinstance(item.value, str) and 'CREATE TABLE' in item.value:
                        blocks[module + '.' + node.name] = item.value
    for name, text in blocks.items():
        (assets / (name + '.sql')).write_text(text.strip() + '\n', encoding='utf-8',newline='\n')


if __name__ == '__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--check-rust':
        check_rust(sys.argv[2])
    else:
        bootstrap()
        with patch.object(catalog_module,'datetime',Clock),patch.object(workflow_data,'datetime',Clock),patch.object(library_query,'datetime',Clock),patch.object(command_batch,'datetime',Clock):
            generate_operations();generate_migrations();generate_folders();generate_preview();generate_predicates();generate_batches();generate_legacy_profile()
        print('Generated catalog operations, migrations, folder platforms, and preview packets.')
