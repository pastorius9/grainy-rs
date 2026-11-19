"""Deterministic cv2 CPU goldens for grainy-imgproc; never imports the Grainy app.

Run with the oracle interpreter and -B. Only golden/imgproc is written.
The installed oracle's CPU optimizations stay enabled, just as in Grainy.
"""
import sys
sys.dont_write_bytecode = True

import ast
from common import Case, GOLDEN, ORACLE, inputs, np, write_json
import cv2

cv2.ocl.setUseOpenCL(False)
DEST = GOLDEN / 'imgproc'
NAMES = []
BORDERS = {'reflect101': cv2.BORDER_REFLECT_101, 'reflect': cv2.BORDER_REFLECT,
           'replicate': cv2.BORDER_REPLICATE, 'constant': cv2.BORDER_CONSTANT,
           'wrap': cv2.BORDER_WRAP}
INTERPOLATIONS = {'linear': cv2.INTER_LINEAR, 'area': cv2.INTER_AREA,
                  'nearest': cv2.INTER_NEAREST}
STANDARD = inputs()
RNG = np.random.default_rng(561311)


def save(op, label, source, result, *, tolerance=1e-6, arrays=None, **params):
    name = f'{op}_{label}'
    assert name not in NAMES, name
    NAMES.append(name)
    if source is not None and source.ndim == 3 and result.ndim == 2:
        result = result[..., None]
    with Case('imgproc', name) as case:
        if source is not None:
            case.array('input', source)
        for key, value in (arrays or {}).items():
            case.array(key, value)
        case.array('output', result)
        case.json(op=op, dtype=None if source is None else str(source.dtype),
                  output_dtype=str(result.dtype), tolerance=tolerance, **params)


def images():
    result = {name: np.ascontiguousarray(a[::9, ::9]) for name, a in STANDARD.items()}
    for h, w in [(1,1), (1,9), (9,1), (2,3), (7,11), (8,12)]:
        result[f'{h}x{w}'] = RNG.random((h,w,3), dtype=np.float32)
    result['signed'] = (RNG.random((7,11,3), dtype=np.float32)-.5)*4
    result['constant'] = np.full((7,11,3), .25, np.float32)
    return result


IMAGES = images()


def aligned64(a):
    """Fix the SIMD alignment prefix without disabling the oracle's CPU paths."""
    storage = np.empty(a.nbytes+64,np.uint8)
    offset = (-storage.ctypes.data) % 64
    result = storage[offset:offset+a.nbytes].view(a.dtype).reshape(a.shape)
    result[...] = a
    return result


def magnitude_reference(a,b):
    a,b = aligned64(a),aligned64(b)
    result = aligned64(np.empty_like(a))
    return cv2.magnitude(a,b,result)


def filters():
    for size in [1,2,3,4,5,6,7,9,13,321]:
        for sigma in [0., .05, .3, 2.5, 40.]:
            for dtype, cvtype in [('f32',cv2.CV_32F),('f64',cv2.CV_64F)]:
                k = cv2.getGaussianKernel(size,sigma,cvtype)[:,0]
                save('kernel',f'{size}_{sigma}_{dtype}',None,k,size=size,sigma=sigma,
                     precision=dtype,tolerance=1e-15 if dtype == 'f64' else 1e-7)
    for label, rgb in IMAGES.items():
        for channels in [1,3]:
            a = rgb[...,0].copy() if channels == 1 else rgb
            tag = f'{label}_c{channels}'
            for sigma in [.05,.3,2.5,40.]:
                save('gaussian',f'{tag}_{sigma}',a,cv2.GaussianBlur(a,(0,0),sigma),
                     size=[0,0],sigma_x=sigma,sigma_y=0.,border='reflect101',tolerance=5e-6 if sigma == 40. else 3e-6)
            for size,sx,sy in [((1,1),0.,0.),((3,3),0.,0.),((5,5),.8,0.),
                               ((7,9),0.,0.),((13,13),1.5,0.),((3,7),.3,2.5)]:
                save('gaussian',f'{tag}_{size[0]}x{size[1]}',a,
                     cv2.GaussianBlur(a,size,sx,sigmaY=sy),size=size,sigma_x=sx,sigma_y=sy,
                     border='reflect101',tolerance=3e-6)
            for size in [1,3,5]:
                save('median',f'{tag}_{size}',a,cv2.medianBlur(a,size),size=size,tolerance=0.)
            for normalize in [False,True]:
                for size,anchor in [((1,1),None),((3,3),None),((4,2),(0,0)),((19,17),None)]:
                    save('box',f'{tag}_{size[0]}_{normalize}',a,
                         cv2.boxFilter(a,-1,size,anchor=anchor or (-1,-1),normalize=normalize),
                         size=size,anchor=anchor,normalize=normalize,border='reflect101',
                         tolerance=2e-4 if not normalize else 1e-6)
            for dx,dy,size,scale in [(1,0,3,1.),(0,1,3,1.),(1,0,3,.17),(1,1,3,.25),(2,0,3,1.),(0,1,1,1.),(1,0,5,.1)]:
                save('sobel',f'{tag}_{dx}_{dy}_{size}_{scale}',a,
                     cv2.Sobel(a,cv2.CV_32F,dx,dy,ksize=size,scale=scale,delta=.125),
                     dx=dx,dy=dy,size=size,scale=scale,delta=.125,border='reflect101',tolerance=2e-5)
            save('integral',tag,a,cv2.integral(a,sdepth=cv2.CV_64F),tolerance=1e-12)
            save('magnitude',tag,a,magnitude_reference(a,a*.37),arrays={'other':a*.37},tolerance=1e-6)
    for border in ['constant','replicate','reflect','reflect101']:
        a = IMAGES['signed']
        save('gaussian',f'border_{border}',a,cv2.GaussianBlur(a,(0,0),2.5,borderType=BORDERS[border]),
             size=[0,0],sigma_x=2.5,sigma_y=0.,border=border,tolerance=2e-6)
        save('box',f'border_{border}',a,cv2.boxFilter(a,-1,(4,6),borderType=BORDERS[border]),
             size=[4,6],anchor=None,normalize=True,border=border)
        save('blur',f'border_{border}',a,cv2.blur(a,(4,3),anchor=(1,0),borderType=BORDERS[border]),
             size=[4,3],anchor=[1,0],border=border)
        for k in [np.array([.3,-.2,.7,.11],np.float32), np.array([-1.,0.,1.],np.float32)]:
            ky = np.array([.2,.3,.5],np.float32)
            save('sep',f'{border}_{len(k)}',a,cv2.sepFilter2D(a,-1,k,ky,delta=.125,borderType=BORDERS[border]),
                 arrays={'kx':k,'ky':ky},anchor=None,delta=.125,border=border,tolerance=2e-6)
    for level in range(5):
        k = np.zeros(4*2**level+1,np.float32)
        k[::2**level] = np.array([1,4,6,4,1],np.float32)/16
        a = IMAGES['wide']
        save('sep',f'wavelet_{level}',a,cv2.sepFilter2D(a,-1,k,k),arrays={'kx':k,'ky':k},
             anchor=None,delta=0.,border='reflect101',tolerance=2e-6)
    for label in ['1x1','1x9','9x1','2x3','7x11','8x12']:
        rgb = np.uint8(np.clip(IMAGES[label],0,1)*255)
        for a in [rgb,rgb[...,0].copy()]:
            save('integral_u8',f'{label}_{a.ndim}',a,cv2.integral(a,sdepth=cv2.CV_32S),tolerance=0.)
            for size in [1,3,5,7]:
                save('median',f'u8_{label}_{a.ndim}_{size}',a,cv2.medianBlur(a,size),size=size,tolerance=0.)


def geometry():
    for label in ['1x1','1x9','9x1','2x3','7x11','8x12','wide','noise']:
        rgb = IMAGES[label]
        for dtype in ['float32','uint8']:
            for channels in [1,3]:
                a = rgb[...,0].copy() if channels == 1 else rgb
                if dtype == 'uint8': a = np.uint8(np.clip(a,0,1)*255)
                h,w = a.shape[:2]
                for interpolation in INTERPOLATIONS:
                    for scale in [.5,1/3,.37,1.,2.,3.7]:
                        size = (max(1,round(w*scale)),max(1,round(h*scale)))
                        save('resize',f'{label}_{dtype}_{channels}_{interpolation}_{scale}',a,
                             cv2.resize(a,size,interpolation=INTERPOLATIONS[interpolation]),
                             size=size,interpolation=interpolation,tolerance=1. if dtype == 'uint8' else 2e-6)
    a = IMAGES['7x11']
    for size in [(17,4),(4,17),(1,1),(22,7),(11,21)]:
        for interpolation in ['linear','area']:
            save('resize',f'mixed_{size[0]}_{size[1]}_{interpolation}',a,cv2.resize(a,size,interpolation=INTERPOLATIONS[interpolation]),size=size,interpolation=interpolation,tolerance=2e-6)
    yy,xx = np.mgrid[:9,:13].astype(np.float32)
    mx = xx*.91-.73+np.sin(yy)*.15
    my = yy*.73-.41+np.cos(xx)*.21
    mx[0,:] = np.array([-100000,-32768,-2.5,-1.015625,-.015625,0,.015625,.046875,.984375,1.015625,10.,32768,100000],np.float32)
    for label in ['1x1','1x9','9x1','7x11','wide']:
        rgb = IMAGES[label]
        for dtype in ['float32','uint8']:
            a = rgb if dtype == 'float32' else np.uint8(np.clip(rgb,0,1)*255)
            for channels in [1,3]:
                a = a[...,0].copy() if channels == 1 else (rgb if dtype == 'float32' else np.uint8(np.clip(rgb,0,1)*255))
                for border in BORDERS:
                    for interpolation in ['linear','nearest']:
                        value = [.25,.5,.75] if dtype == 'float32' else [32,64,128]
                        save('remap',f'{label}_{dtype}_{channels}_{border}_{interpolation}',a,
                             cv2.remap(a,mx,my,INTERPOLATIONS[interpolation],borderMode=BORDERS[border],borderValue=tuple(value)),
                             arrays={'mx':mx,'my':my},interpolation=interpolation,border=border,
                             border_value=value,tolerance=1. if dtype == 'uint8' else 1e-6)
    for label,center,angle,scale in [('identity',(0.,0.),0.,1.),('rotate',(4.5,3.),-6.,1.),('scale',(3.2,1.7),29.7,.72),('zero',(1.,2.),90.,0.)]:
        m = cv2.getRotationMatrix2D(center,angle,scale)
        save('rotation',label,None,m,center=center,angle=angle,scale=scale,tolerance=1e-12)
    srcpts = np.array([[.3,.7],[10.2,-.2],[9.1,6.8],[-.4,5.9]],np.float32)
    dstpts = np.array([[0,0],[10,0],[10,6],[0,6]],np.float32)
    for label,s,t in [('skew',srcpts,dstpts),('identity',dstpts,dstpts),('singular',np.zeros((4,2),np.float32),dstpts)]:
        save('perspective_matrix',label,None,cv2.getPerspectiveTransform(s,t),arrays={'src_points':s,'dst_points':t},tolerance=1e-10,expected_error=label == 'singular')
    matrices = [('identity',np.eye(3)),('projective',cv2.getPerspectiveTransform(srcpts,dstpts)),
                ('zero',np.zeros((3,3))),('horizon',np.array([[1.,0.,.3],[0.,1.,-.7],[.1,0.,0.]]))]
    for dtype in ['float32','uint8']:
        a = IMAGES['7x11'] if dtype == 'float32' else np.uint8(IMAGES['7x11']*255)
        for interpolation in ['linear','nearest']:
            for inverse in [False,True]:
                for border in ['constant','reflect101','replicate']:
                    flags = INTERPOLATIONS[interpolation] | (cv2.WARP_INVERSE_MAP if inverse else 0)
                    value = [.6,.4,.2] if dtype == 'float32' else [90,60,30]
                    for label,m in matrices:
                        # OpenCV 5's continuous linear path hangs for a singular
                        # inverse/zero denominator with reflective borders.
                        # Rust rejects these invalid matrices instead of hanging.
                        if label in ('zero','horizon'):
                            continue
                        save('warp_perspective',f'{dtype}_{interpolation}_{inverse}_{border}_{label}',a,
                             cv2.warpPerspective(a,m,(13,9),flags=flags,borderMode=BORDERS[border],borderValue=tuple(value)),
                             arrays={'matrix':m},size=[13,9],interpolation=interpolation,border=border,
                             border_value=value,inverse=inverse,tolerance=1. if dtype == 'uint8' else 1e-6)
                    for label,m in [('rotate',cv2.getRotationMatrix2D((4.5,3.),-6.,1.)),('translation',np.array([[1.,0.,.484375],[0.,1.,-.515625]])),('zero',np.zeros((2,3)))]:
                        if label == 'zero':
                            continue
                        save('warp_affine',f'{dtype}_{interpolation}_{inverse}_{border}_{label}',a,
                             cv2.warpAffine(a,m,(13,9),flags=flags,borderMode=BORDERS[border],borderValue=tuple(value)),
                             arrays={'matrix':m},size=[13,9],interpolation=interpolation,border=border,
                             border_value=value,inverse=inverse,tolerance=1. if dtype == 'uint8' else 1e-6)


def colours():
    edge = np.array([[0,0,0],[1,1,1],[.5,.5,.5],[1,0,0],[0,1,0],[0,0,1],
                     [1,1,0],[1,0,1],[0,1,1],[.2,.2+1e-8,.2],[.5,.5+2e-7,.5],
                     [-.2,-.1,.4],[3,2,1],[1.2,.5,-.1]],np.float32)[None]
    for label,rgb in list(IMAGES.items())+[('edge',edge)]:
        for dtype in ['float32','uint8']:
            a = rgb if dtype == 'float32' else np.uint8(np.clip(rgb,0,1)*255)
            for code in ['RGB2HLS','RGB2HSV','RGB2GRAY']:
                out = cv2.cvtColor(a,getattr(cv2,'COLOR_'+code))
                save('color',f'{label}_{dtype}_{code}',a,out,code=code,tolerance=1. if dtype == 'uint8' else 6e-5)
                if code != 'RGB2GRAY':
                    inverse = 'HLS2RGB' if code == 'RGB2HLS' else 'HSV2RGB'
                    save('color',f'{label}_{dtype}_{inverse}',out,cv2.cvtColor(out,getattr(cv2,'COLOR_'+inverse)),
                         code=inverse,tolerance=1. if dtype == 'uint8' else 3e-6)
    for code in ['HLS2RGB','HSV2RGB']:
        a = np.array([[h,.3,.7] for h in [-720,-60,-.01,0,59.999,60,120,180,240,300,359.99,360,720]],np.float32)[None]
        save('color',f'hue_wrap_{code}',a,cv2.cvtColor(a,getattr(cv2,'COLOR_'+code)),code=code,tolerance=2e-6)


def morphology():
    shapes = {'ellipse':cv2.MORPH_ELLIPSE,'rect':cv2.MORPH_RECT,'cross':cv2.MORPH_CROSS}
    for kind,cvkind in shapes.items():
        for size in [(1,1),(1,7),(7,1),(2,3),(4,6),(5,5),(9,7),(17,19)]:
            for at in [None,(0,0)]:
                save('structuring',f'{kind}_{size[0]}_{size[1]}_{at}',None,
                     cv2.getStructuringElement(cvkind,size,at or (-1,-1)),kind=kind,size=size,anchor=at,tolerance=0.)
    operations = {'erode':cv2.MORPH_ERODE,'dilate':cv2.MORPH_DILATE,'open':cv2.MORPH_OPEN,
                  'close':cv2.MORPH_CLOSE,'gradient':cv2.MORPH_GRADIENT,
                  'tophat':cv2.MORPH_TOPHAT,'blackhat':cv2.MORPH_BLACKHAT}
    for label in ['1x1','1x9','9x1','2x3','7x11','signed']:
        for dtype in ['float32','uint8']:
            for channels in [1,3]:
                rgb = IMAGES[label]
                a = rgb if channels == 3 else rgb[...,0].copy()
                if dtype == 'uint8': a = np.uint8(np.clip(a,0,1)*255)
                for kind,cvkind in shapes.items():
                    k = cv2.getStructuringElement(cvkind,(4,5))
                    for op,cvop in operations.items():
                        save('morph',f'{label}_{dtype}_{channels}_{kind}_{op}',a,cv2.morphologyEx(a,cvop,k),
                             arrays={'kernel':k},operation=op,anchor=None,iterations=1,border=None,tolerance=0.)
    a = IMAGES['signed']
    k = np.array([[1,0,0],[0,1,0],[1,0,1]],np.uint8)
    for border in ['constant','replicate','reflect101','reflect']:
        for n in [0,1,2]:
            for op,cvop in operations.items():
                save('morph',f'border_{border}_{n}_{op}',a,
                     cv2.morphologyEx(a,cvop,k,anchor=(0,0),iterations=n,borderType=BORDERS[border],borderValue=(.2,.3,.4)),
                     arrays={'kernel':k},operation=op,anchor=[0,0],iterations=n,border=border,border_value=[.2,.3,.4],tolerance=0.)


def coverage():
    """Full standard inputs, SIMD-sized rows, and the sky mask's five-channel fallback."""
    for label,rgb in STANDARD.items():
        for channels in [1,3]:
            a = rgb[...,0].copy() if channels == 1 else rgb
            save('gaussian',f'full_{label}_{channels}',a,cv2.GaussianBlur(a,(0,0),2.5),
                 size=[0,0],sigma_x=2.5,sigma_y=0.,border='reflect101',tolerance=3e-6)
    yy,xx = np.mgrid[:17,:37].astype(np.float32)
    mx,my = xx*.61-.173, yy*.73-.219
    for c in [1,2,3,4,5]:
        rgb = RNG.random((13,23,c),dtype=np.float32)
        for dtype in ['float32','uint8']:
            a = rgb if dtype == 'float32' else np.uint8(rgb*255)
            for interpolation in ['linear','nearest']:
                for border in ['replicate','reflect101','constant']:
                    # OpenCV's legacy fallback limits nonzero Scalar borders to <=4 channels.
                    value = [0.,0.,0.,0.]
                    for interleaved in [False,True]:
                        op = 'remap_interleaved' if interleaved else 'remap'
                        xy = np.dstack([mx,my])
                        out = cv2.remap(a,xy if interleaved else mx,None if interleaved else my,
                                        INTERPOLATIONS[interpolation],borderMode=BORDERS[border])
                        save(op,f'vector_{dtype}_{c}_{interpolation}_{border}',a,out,
                             arrays={'map':xy} if interleaved else {'mx':mx,'my':my},interpolation=interpolation,
                             border=border,border_value=value,tolerance=1. if dtype == 'uint8' else 1e-6)
                for op in ['warp_affine','warp_perspective']:
                    m = cv2.getRotationMatrix2D((10.3,7.1),17.1,.91)
                    if op == 'warp_perspective':
                        m = np.vstack([m,[.0013,-.0007,1.]])
                    out = (cv2.warpAffine if op == 'warp_affine' else cv2.warpPerspective)(a,m,(37,17),flags=INTERPOLATIONS[interpolation],borderMode=cv2.BORDER_REFLECT_101)
                    save(op,f'vector_{dtype}_{c}_{interpolation}',a,out,arrays={'matrix':m},size=[37,17],
                         interpolation=interpolation,border='reflect101',inverse=False,tolerance=1. if dtype == 'uint8' else 2e-6)
    for width in [5,8,9,16,19]:
        for code in ['HLS2RGB','HSV2RGB']:
            a = np.tile(np.array([-60.,.3,.7],np.float32),(2,width,1))
            save('color',f'negative_hue_{width}_{code}',a,cv2.cvtColor(a,getattr(cv2,'COLOR_'+code)),code=code,tolerance=2e-6)
    for code in ['RGB2HLS','RGB2HSV','RGB2GRAY']:
        a = STANDARD['colours']
        save('color',f'full_colours_{code}',a,cv2.cvtColor(a,getattr(cv2,'COLOR_'+code)),code=code,tolerance=6e-5)
    for dtype in ['float32','uint8']:
        a = RNG.random((2,9,4),dtype=np.float32)
        if dtype == 'uint8': a = np.uint8(a*255)
        save('color',f'rgba_{dtype}',a,cv2.cvtColor(a,cv2.COLOR_RGB2GRAY),code='RGB2GRAY',tolerance=0. if dtype == 'uint8' else 1e-6)


def inventory():
    calls = []
    for path in sorted((ORACLE/'luma').glob('*.py')):
        if path.name == 'selftest.py':
            continue
        text = path.read_text(encoding='utf-8-sig')
        tree = ast.parse(text)
        for node in ast.walk(tree):
            if isinstance(node,ast.Call) and isinstance(node.func,ast.Attribute) and isinstance(node.func.value,ast.Name) and node.func.value.id == 'cv2':
                calls.append({'file':'luma/'+path.name,'line':node.lineno,'function':node.func.attr,
                              'call':ast.get_source_segment(text,node)})
            elif (isinstance(node,ast.Call) and isinstance(node.func,ast.Attribute) and node.func.attr=='detect'
                  and isinstance(node.func.value,ast.Call) and isinstance(node.func.value.func,ast.Attribute)
                  and isinstance(node.func.value.func.value,ast.Name) and node.func.value.func.value.id=='cv2'):
                calls.append({'file':'luma/'+path.name,'line':node.lineno,
                              'function':node.func.value.func.attr+'.detect','call':ast.get_source_segment(text,node)})
    write_json(DEST/'oracle_calls.json',calls)


def tier2_masks():
    rng = np.random.default_rng(90113)
    h,w=23,31
    masks = {'empty':np.zeros((h,w),np.uint8),'full':np.ones((h,w),np.uint8)}
    m=np.zeros((h,w),np.uint8);m[11,15]=255;masks['pixel']=m
    m=np.zeros((h,w),np.uint8);m[6:16,9:21]=1;masks['hole']=m
    m=np.zeros((h,w),np.uint8);m[:7,:11]=255;m[-5:,-4:]=2;masks['border']=m
    m=np.zeros((h,w),np.uint8);m[2:21,2:29]=1;m[5:18,5:26]=0;m[8:15,8:23]=1;m[10:13,11:20]=0;masks['nested']=m
    m=np.zeros((h,w),np.uint8);m[::2,::2]=1;masks['many']=m
    masks['ties']=np.uint8(np.indices((h,w)).sum(0)%2)
    masks['random']=np.uint8(rng.random((h,w))>.63)
    masks['single']=np.ones((1,1),np.uint8)
    masks['row']=np.uint8([[0,1,1,0,0,1,0,1,1]])
    masks['column']=masks['row'].T.copy()
    m=np.zeros((2,2),np.uint8);m[0,0]=255;masks['tiny_corner']=m
    m=np.ones((3,4),np.uint8);m[1,2]=0;masks['only_known']=m
    return masks


def tier2():
    """Packed function-level cases: preserve dtype, reduce filesystem overhead."""
    rng=np.random.default_rng(21113)
    names=[]
    masks=tier2_masks()
    for op in ['inpaint','distance','components','flood']:
        name='tier2_'+op;names.append(name)
        variants=[]
        with Case('imgproc',name) as case:
            source_masks=dict(masks)
            if op=='components':
                source_masks['parallel_many']=np.uint8(rng.random((129,177))>.73)
            if op=='distance':
                m=np.ones((257,389),np.uint8);m[128,194]=0;source_masks['large_center']=m
                m=np.ones((129,193),np.uint8);m[0,0]=0;source_masks['large_corner']=m
            for label,mask in source_masks.items():
                h,w=mask.shape
                if op=='inpaint':
                    if min(h,w)<2: continue
                    yy,xx=np.mgrid[:h,:w].astype(np.float32)
                    images={'ramp':xx/w+yy/h,'noise':rng.random((h,w),dtype=np.float32),
                            'signed':(rng.random((h,w),dtype=np.float32)-.5)*100,
                            'constant':np.full((h,w),.375,np.float32)}
                    for image,a in images.items():
                        for radius in ([0.,1.,2.5,3.,5.,100.] if label=='pixel' else [1.,2.5,3.,5.,100.]):
                            i=len(variants);case.array(f'input_{i}',a);case.array(f'mask_{i}',mask)
                            case.array(f'output_{i}',cv2.inpaint(a,mask,radius,cv2.INPAINT_NS))
                            variants.append(dict(label=label+'_'+image,radius=radius,tolerance=0.))
                elif op=='distance':
                    for size in [3,5]:
                        i=len(variants);case.array(f'input_{i}',mask)
                        case.array(f'output_{i}',cv2.distanceTransform(mask,cv2.DIST_L2,size))
                        variants.append(dict(label=label,size=size,tolerance=2e-4))
                elif op=='components':
                    for connectivity in [4,8]:
                        i=len(variants);case.array(f'input_{i}',mask)
                        count,labels,stats,centroids=cv2.connectedComponentsWithStats(mask,connectivity=connectivity)
                        n2,l2=cv2.connectedComponents(mask,connectivity=connectivity)
                        assert count==n2 and np.array_equal(labels,l2)
                        case.array(f'labels_{i}',labels);case.array(f'stats_{i}',stats);case.array(f'centroids_{i}',centroids)
                        variants.append(dict(label=label,connectivity=connectivity,count=count,tolerance=0.))
                else:
                    for seed in [(0,0),(w//2,h//2),(w-1,h-1)]:
                        for value in [0,2]:
                            i=len(variants);case.array(f'input_{i}',mask)
                            count,out,fill_mask,rect=cv2.floodFill(mask.copy(),None,seed,value)
                            case.array(f'output_{i}',out);case.array(f'mask_{i}',fill_mask)
                            variants.append(dict(label=label,seed=seed,value=value,count=count,rect=rect,tolerance=0.))
            case.json(op=op,variants=variants)
    names.append('tier2_contours')
    with Case('imgproc','tier2_contours') as case:
        variants=[]
        for label,mask in masks.items():
            for mode in ['external','ccomp']:
                i=len(variants);case.array(f'input_{i}',mask)
                contours,hierarchy=cv2.findContours(mask,cv2.RETR_EXTERNAL if mode=='external' else cv2.RETR_CCOMP,cv2.CHAIN_APPROX_SIMPLE)
                measurements=[]
                for c in contours:
                    measurements.append(dict(area=cv2.contourArea(c),signed=cv2.contourArea(c,True),
                        closed=cv2.arcLength(c,True),opened=cv2.arcLength(c,False),rect=cv2.boundingRect(c),
                        convex=cv2.isContourConvex(c),hull=cv2.convexHull(c).reshape(-1,2),
                        hull_cw=cv2.convexHull(c,clockwise=True).reshape(-1,2)))
                variants.append(dict(label=label,mode=mode,contours=[c.reshape(-1,2) for c in contours],
                    hierarchy=[] if hierarchy is None else hierarchy[0],measurements=measurements))
        case.json(op='contours',variants=variants)
    names.append('tier2_polygon')
    with Case('imgproc','tier2_polygon') as case:
        variants=[]
        polygons=[[],[[1,2]],[[1,2],[3,4]],[[0,0],[1,0],[1,1],[0,1]],
            [[0,0],[.5,0],[1,0],[1,1],[0,1]],[[0,0],[1,1],[0,1],[1,0]],
            [[-.1,-.2],[.9,-.1],[.8,.9],[-.2,.8]]]
        for i,p in enumerate(polygons):
            p=np.array(p,np.float32).reshape(-1,1,2)
            variants.append(dict(points=p.reshape(-1,2),area=cv2.contourArea(p),signed=cv2.contourArea(p,True),
                closed=cv2.arcLength(p,True),opened=cv2.arcLength(p,False),rect=cv2.boundingRect(p),convex=cv2.isContourConvex(p)))
        case.json(op='polygon',variants=variants)
    names.append('tier2_hull')
    with Case('imgproc','tier2_hull') as case:
        variants=[]
        point_sets=[np.empty((0,2),np.int32),np.array([[4,3]]*8,np.int32),
            np.array([[3,3],[2,2],[1,1],[2,2],[-1,-1],[9,9]],np.int32)]
        for count in [3,4,9,23,80]:
            for _ in range(5): point_sets.append(rng.integers(-25,26,(count,2),dtype=np.int32))
        for points in point_sets:
            hull=cv2.convexHull(points);cw=cv2.convexHull(points,clockwise=True)
            variants.append(dict(points=points,hull=[] if hull is None else hull.reshape(-1,2),
                hull_cw=[] if cw is None else cw.reshape(-1,2)))
        case.json(op='hull',variants=variants)
    for op in ['drawing','moments','absdiff','laplacian','bilateral','template']:
        names.append('tier2_'+op);variants=[]
        with Case('imgproc','tier2_'+op) as case:
            if op in ['drawing','moments']:
                for label,mask in masks.items():
                    if op=='moments':
                        for binary in [False,True]:
                            i=len(variants);case.array(f'input_{i}',mask)
                            variants.append(dict(label=label,binary=binary,moments=cv2.moments(mask,binary)))
                    else:
                        contours,_=cv2.findContours(mask,cv2.RETR_CCOMP,cv2.CHAIN_APPROX_SIMPLE)
                        for thickness in [-1,1]:
                            i=len(variants);a=np.zeros_like(mask);case.array(f'input_{i}',a)
                            case.array(f'output_{i}',cv2.drawContours(a.copy(),contours,-1,7,thickness))
                            variants.append(dict(label=label,contours=[c.reshape(-1,2) for c in contours],thickness=thickness))
                if op=='drawing':
                    for label,points in [('clipped',[[-9,2],[24,-4],[40,17],[6,29]]),
                        ('bowtie',[[1,2],[27,21],[5,18],[22,0]]),('outside',[[-8,7],[-3,7],[-2,17]])]:
                        for thickness in [-1,1]:
                            i=len(variants);a=np.zeros((23,31),np.uint8);case.array(f'input_{i}',a)
                            contours=[np.array(points,np.int32).reshape(-1,1,2)]
                            case.array(f'output_{i}',cv2.drawContours(a.copy(),contours,-1,7,thickness))
                            variants.append(dict(label=label,contours=[points],thickness=thickness))
            else:
                images={k:np.ascontiguousarray(v[::5,::5]) for k,v in STANDARD.items()}
                images.update(constant=np.full((11,17,3),.5,np.float32),zero=np.zeros((5,7,3),np.float32),
                    signed=(rng.random((13,21,3),dtype=np.float32)-.5)*4,tiny=rng.random((2,3,3),dtype=np.float32))
                for label,rgb in images.items():
                    for cn in [1,3]:
                        a=rgb[...,0].copy() if cn==1 else rgb
                        if op=='absdiff':
                            for dtype in ['float32','uint8']:
                                src=a if dtype=='float32' else np.uint8(np.clip(a*255,0,255))
                                other=np.flip(src,axis=1).copy();i=len(variants)
                                case.array(f'input_{i}',src);case.array(f'other_{i}',other);case.array(f'output_{i}',cv2.absdiff(src,other))
                                variants.append(dict(label=label,dtype=dtype,tolerance=0.))
                        elif op=='laplacian':
                            for border in ['reflect101','reflect','replicate','constant']:
                                i=len(variants);case.array(f'input_{i}',a);case.array(f'output_{i}',cv2.Laplacian(a,cv2.CV_32F,borderType=BORDERS[border]))
                                variants.append(dict(label=label,border=border,tolerance=1e-5))
                        elif op=='bilateral' and cn==1:
                            for d,sc,ss in [(1,2.,.05),(3,2.,1.),(7,10.,2.5),(13,18.,4.),(0,2.,1.5),(3,0.,1.)]:
                                i=len(variants);luma=a*25
                                case.array(f'input_{i}',luma);case.array(f'output_{i}',cv2.bilateralFilter(luma,d,sc,ss))
                                variants.append(dict(label=label,d=d,sc=sc,ss=ss,tolerance=2e-4))
                        elif op=='template':
                            for size in [(1,1),(min(3,a.shape[0]),min(4,a.shape[1])),a.shape[:2]]:
                                for method in ['sqdiff','ccoeff']:
                                    t=a[:size[0],:size[1]].copy();i=len(variants)
                                    case.array(f'input_{i}',a);case.array(f'template_{i}',t)
                                    case.array(f'output_{i}',cv2.matchTemplate(a,t,cv2.TM_SQDIFF if method=='sqdiff' else cv2.TM_CCOEFF_NORMED))
                                    variants.append(dict(label=label,method=method,tolerance=2e-4))
                if op=='template':
                    ramp=np.arange(15,dtype=np.float32).reshape(3,5)/15
                    special=[('swapped_equal_axis',ramp[:2].copy(),ramp),
                             ('swapped_both_axes',ramp[:2,:3].copy(),ramp),
                             ('flat_image',np.ones((5,7),np.float32),ramp),
                             ('anticorrelated',ramp,1-ramp)]
                    for label,a,t in special:
                        for method in ['sqdiff','ccoeff']:
                            i=len(variants);case.array(f'input_{i}',a);case.array(f'template_{i}',t)
                            case.array(f'output_{i}',cv2.matchTemplate(a,t,cv2.TM_SQDIFF if method=='sqdiff' else cv2.TM_CCOEFF_NORMED))
                            variants.append(dict(label=label,method=method,tolerance=2e-4))
            case.json(op=op,variants=variants)
    names.append('tier2_lines')
    with Case('imgproc','tier2_lines') as case:
        variants=[]
        drawings={'empty':np.zeros((96,128),np.uint8),'full':np.full((96,128),255,np.uint8),
            'random_noise':rng.integers(0,256,(67,91),dtype=np.uint8),
            'single':np.uint8([[255]]),'row':np.uint8([[0,255,0,255,255,0,0,0,255]])}
        a=np.zeros((96,128),np.uint8);a[10,5:120]=255;a[35,10:100]=255;a[60:90,35]=255;drawings['axes']=a
        a=np.zeros((96,128),np.uint8)
        for p,q in [((3,7),(120,70)),((5,90),(110,15)),((0,20),(127,22))]: cv2.line(a,p,q,255,1)
        drawings['crossing']=a
        a=np.zeros((96,128),np.uint8);cv2.rectangle(a,(11,15),(112,81),210,2);drawings['rectangle']=a
        a=np.zeros((96,128),np.uint8);cv2.ellipse(a,(64,48),(50,30),0,0,360,255,2);drawings['ellipse']=a
        a=np.zeros((96,128),np.uint8);a[30,5:120]=255;a[30,40:42]=0;a[30,71:76]=0;drawings['gaps']=a
        a=np.zeros((67,91),np.uint8)
        for p,q in [((0,0),(90,66)),((0,66),(90,0)),((3,20),(88,43))]:cv2.line(a,p,q,127,2,cv2.LINE_AA)
        drawings['antialias']=a
        for index in range(12):
            a=rng.integers(0,12,(79+index,109+index),dtype=np.uint8)
            for _ in range(5):
                p=tuple(rng.integers(0,min(a.shape),2));q=tuple(rng.integers(0,min(a.shape),2))
                cv2.line(a,p,q,int(rng.integers(40,256)),int(rng.integers(1,4)))
            drawings[f'textured_lines_{index}']=a
        drawings.update({k:np.uint8(np.clip(v[...,0],0,1)*255) for k,v in STANDARD.items()})
        for label,a in drawings.items():
            i=len(variants);case.array(f'input_{i}',a)
            blurred=cv2.GaussianBlur(a,(7,7),.6/.8);case.array(f'blurred_{i}',blurred)
            case.array(f'scaled_{i}',cv2.resize(blurred,None,fx=.8,fy=.8,interpolation=cv2.INTER_LINEAR_EXACT))
            segments=cv2.createLineSegmentDetector(cv2.LSD_REFINE_STD).detect(a)[0]
            h,w=a.shape;hough=[]
            for rho,theta,threshold,length,gap in [(1.,np.pi/720,max(20,round(.12*w)),.25*w,max(2,round(.01*w))),
                (1.,np.pi/180,8,5.,0.),(2.,np.pi/90,12,10.,4.)]:
                lines=cv2.HoughLinesP(a,rho,theta,threshold,minLineLength=length,maxLineGap=gap)
                hough.append(dict(rho=rho,theta=theta,threshold=threshold,length=length,gap=gap,
                    lines=[] if lines is None else lines.reshape(-1,4)))
            variants.append(dict(label=label,segments=[] if segments is None else segments.reshape(-1,4),hough=hough,tolerance=2e-4))
        case.json(op='lines',variants=variants)
    write_json(DEST/'manifest_tier2.json',dict(opencv=cv2.__version__,numpy=np.__version__,
                opencl=False,cpu_features=cv2.getCPUFeaturesLine(),
                optimized=cv2.useOptimized(),ipp=cv2.ipp.useIPP(),cases=names))
    print(f'Generated tier 2: {len(names)} packed case folders',flush=True)


if __name__ == '__main__':
    inventory()
    if '--tier2' in sys.argv:
        tier2()
        sys.exit(0)
    filters()
    print('Filters generated',flush=True)
    geometry()
    print('Geometry generated',flush=True)
    colours()
    print('Colours generated',flush=True)
    morphology()
    coverage()
    write_json(DEST/'manifest.json',{'opencv':cv2.__version__,'numpy':np.__version__,
                                   'opencl':False,'optimized':cv2.useOptimized(),
                                   'cpu_features':cv2.getCPUFeaturesLine(),
                                   'ipp':cv2.ipp.useIPP(),
                                   'cases':NAMES})
    print(f'Generated {len(NAMES)} cases with cv2 {cv2.__version__}; only {DEST} was written.')
    tier2()
