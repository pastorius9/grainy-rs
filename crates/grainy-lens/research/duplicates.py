"""Measure database duplicate identity through the public Database interface."""
from probe import *
for base in [1,1.5,3,8]:
    for delta in [.0008,.001,.00101,.005,.009,.00999,.01,.01001,.0149,.015,.0151,.02,.029,.03,.08,.1]:
        xml='<lensdatabase><mount><name>A</name></mount>'
        for crop in [base,base+delta]:
            xml+=f'<lens><maker>Test</maker><model>Test 50mm</model><mount>A</mount><cropfactor>{crop}</cropfactor><calibration><distortion model="poly3" focal="50" k1=".01"/></calibration></lens>'
        xml+='</lensdatabase>'
        db=lf.Database(xml=xml,load_common=False,load_bundled=False)
        print(base,delta,[l.crop_factor for l in db.lenses])
for maker,model,mount in [('Test','Test 50mm','B'),('TEST','Test 50mm','A'),('Test','TEST 50mm','A')]:
    xml='<lensdatabase>'
    for ma,mo,mt in [('Test','Test 50mm','A'),(maker,model,mount)]:
        xml+=f'<lens><maker>{ma}</maker><model>{mo}</model><mount>{mt}</mount><cropfactor>1</cropfactor></lens>'
    db=lf.Database(xml=xml+'</lensdatabase>',load_common=False,load_bundled=False)
    print('identity',maker,model,mount,[(l.maker,l.model,l.mounts) for l in db.lenses])
