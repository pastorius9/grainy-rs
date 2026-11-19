"""Public scalar coordinate outputs distinguish floating point evaluation orders."""
from probe import *
for model,attrs,terms in [('poly3','k1="0.008631"',[.008631]),('poly5','k1="-0.030571633" k2="0.004658548"',[-.030571633,.004658548]),('ptlens','a="0.00641" b="-0.0083" c="0.00228"',[.00641,-.0083,.00228])]:
    db,lens=synthetic(f'<distortion model="{model}" focal="50" {attrs}/>',aspect='1')
    m=lf.Modifier(lens,1,3,3);m.initialize(50,4,scale=1,pixel_format=np.float32,flags=int(lf.ModifyFlags.DISTORTION))
    ts=np.array(terms,np.float32);scores=np.zeros(10,int);worst=np.zeros(10)
    for x,y in np.random.default_rng(14).uniform(-2,2,(5000,2)):
        xu=np.float32(x+1);yu=np.float32(y+1);x=np.float32(float(xu)-1);y=np.float32(float(yu)-1)
        obs=m.apply_geometry_distortion(xu=xu,yu=yu,width=1,height=1)[0,0]
        r2=np.float32(x*x+y*y);r=np.sqrt(r2);k=ts[0]
        if model=='poly3':
            fs=[np.float32(1-k+k*r2),np.float32(1+k*(r2-1)),np.float32(1-float(k)+float(k)*float(r2)),np.float32(1+float(k)*(float(r2)-1))]
        elif model=='poly5':
            k2=ts[1];fs=[np.float32(1-k+k*r2),np.float32(1+r2*(k+k2*r2)),np.float32(1+float(k*r2+k2*r2*r2)),np.float32(1+float(k)*float(r2)+float(k2)*float(r2)**2),np.float32(1+(k*r2+k2*(r2*r2))),np.float32(1+float(k*r2)+float(k2*r2*r2)),np.float32(1+float(k*r2)+float(k2*(r2*r2)))]
        else:
            a,b,c=ts;d=np.float32(1-a-b-c);dd=np.float32(1-(a+b+c))
            rd=float(np.sqrt(float(r2)))
            ddd=np.float32(1-float(a)-float(b)-float(c))
            fs=[np.float32(((a*r+b)*r+c)*r+d),np.float32(d+a*r2*r+b*r2+c*r),np.float32(d+r*(c+r*(b+a*r))),np.float32(dd+a*r2*r+b*r2+c*r),np.float32(((a*r+b)*r+c)*r+dd),np.float32(((float(a)*rd+float(b))*rd+float(c))*rd+float(dd)),np.float32(((a*r+b)*r+c)*r+1-float(a+b+c)),np.float32(((a*r+b)*r+c)*r+ddd),np.float32(a*r*r*r+b*r*r+c*r+dd),np.float32(a*r2*r+b*r2+c*r+dd)]
        for i,f in enumerate(fs):
            out=np.array([float(np.float32(x*f))+1,float(np.float32(y*f))+1],np.float32)
            e=np.max(abs(out-obs));scores[i]+=int(e==0);worst[i]=max(e,worst[i])
    print(model,'scores',scores,'worst',worst)
