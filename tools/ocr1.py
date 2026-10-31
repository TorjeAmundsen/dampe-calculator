import numpy as np, cv2, pickle
def cells(a):
    # segment by the gaps in the column profile: glyphs touch nothing horizontally
    on=(a>100).any(axis=0); xs=[];i=0
    while i<len(on):
        if on[i]:
            j=i
            while j<len(on) and on[j]: j+=1
            xs.append((i,j)); i=j
        else: i+=1
    # merged glyphs: split any wide run on the 38 px pitch
    out=[]
    for x0,x1 in xs:
        n=max(1,round((x1-x0+4)/38))
        for k in range(n):
            out.append((x0+(x1-x0)*k//n, x0+(x1-x0)*(k+1)//n))
    return out
def norm(a,x0,x1):
    g=a[6:40,x0:x1]; return cv2.resize(g,(16,20),interpolation=cv2.INTER_AREA).astype(np.float32)
if __name__=='__main__':
    samples=[];bad=0
    for c in ('c1','c2'):
        w=np.load(c+'_watch.npy',mmap_mode='r')
        wd=np.abs(np.diff(np.array(w,dtype=np.int16),axis=0)).sum(axis=(1,2))
        for i in [0]+(np.where(wd>2000)[0]+1).tolist():
            a=np.array(w[i]); xs=cells(a)
            if len(xs)!=10: bad+=1; continue
            for x0,x1 in xs[2:]: samples.append(norm(a,x0,x1))
    print('bad seg',bad,'samples',len(samples))
    cl=[];cnt=[]
    for s in samples:
        if cl:
            d=np.abs(np.array(cl)-s).mean(axis=(1,2)); j=int(d.argmin())
            if d[j]<25: cnt[j]+=1; continue
        cl.append(s);cnt.append(1)
    print(len(cl),cnt)
    sheet=np.hstack([cv2.copyMakeBorder(c.astype(np.uint8),2,2,2,2,cv2.BORDER_CONSTANT,value=80) for c in cl])
    cv2.imwrite('atlas.png',cv2.resize(sheet,None,fx=3,fy=3,interpolation=cv2.INTER_NEAREST))
    pickle.dump(cl,open('cl.pkl','wb'))
