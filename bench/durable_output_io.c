/* macOS-only profiling/fault interposer. Never loaded by normal acquisition.
 * Measures syscall-facing time, not physical device traffic. */
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <errno.h>
#include <pthread.h>
#define INTERPOSE(repl,orig) __attribute__((used)) static struct {const void *r,*o;} ip_##orig __attribute__((section("__DATA,__interpose")))={(const void*)(repl),(const void*)(orig)};
typedef struct {
    unsigned long opens, creates, writes, bytes, reads, read_bytes, fsyncs, fullsyncs, errors;
    double open_s, write_s, read_s, fsync_s, fullsync_s;
} Stats;
static Stats stats[4]; /* source, output files, manifest, directories */
static unsigned long mkdirs, renames;
static double mkdir_s, rename_s;
static int committed;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec*1e-9; }
static int path_kind(const char *p) {
    const char *root=getenv("TR_IO_OUTPUT"), *source=getenv("TR_IO_SOURCE");
    if(root && !strncmp(p,root,strlen(root))) {
        const char *base=strrchr(p,'/');
        if(base && (!strcmp(base+1,"stages.jsonl") || !strcmp(base+1,"request.json") || !strcmp(base+1,"io.json"))) return -1;
        return base && !strcmp(base+1,"manifest.json") ? 2 : 1;
    }
    return source && !strncmp(p,source,strlen(source)) ? 0 : -1;
}
static int kind(int fd) {
    int saved=errno, k=-1; char p[4096]; struct stat st;
    if(fd>=0 && !fcntl(fd,F_GETPATH,p)) {
        k=path_kind(p);
        const char *root=getenv("TR_IO_OUTPUT");
        size_t len=strlen(p);
        int ancestor=root && !strncmp(root,p,len) && (len==1 || root[len]=='/' || root[len]=='\0');
        if((k>0 || ancestor) && !fstat(fd,&st) && S_ISDIR(st.st_mode)) k=3;
    }
    errno=saved; return k;
}
/* Pause before one real operation until the test releases it. Signals are sent
 * only after the marker exists, so each fault has an observable phase. */
static int fault(const char *point) {
    const char *wanted=getenv("TR_FAULT_POINT");
    static int fired=0;
    if(!wanted || strcmp(point,wanted) || __sync_lock_test_and_set(&fired,1)) return 0;
    const char *marker=getenv("TR_FAULT_MARKER");
    if(marker) { int fd=open(marker,O_WRONLY|O_CREAT|O_TRUNC,0600); if(fd>=0)close(fd); }
    const char *action=getenv("TR_FAULT_ACTION");
    if(action && !strcmp(action,"pause")) {
        char release[4096]; snprintf(release,sizeof(release),"%s.release",marker);
        double deadline=now()+30;
        while(access(release,F_OK) && now()<deadline) usleep(2000);
        return 0;
    }
    errno=EIO; return 1;
}
static void record(int k,int op,ssize_t n,double sec,int create) {
    if(k<0)return;
    int saved=errno; pthread_mutex_lock(&lock); Stats *s=&stats[k];
    if(n<0)s->errors++;
    switch(op) {
        case 0:s->opens++;s->creates+=create;s->open_s+=sec;break;
        case 1:s->writes++;s->bytes+=n>0?n:0;s->write_s+=sec;break;
        case 2:s->reads++;s->read_bytes+=n>0?n:0;s->read_s+=sec;break;
        case 3:s->fsyncs++;s->fsync_s+=sec;break;
        case 4:s->fullsyncs++;s->fullsync_s+=sec;break;
    }
    pthread_mutex_unlock(&lock); errno=saved;
}
static int write_fault(int fd,int k) {
    if(k<=0)return 0;
    if(fault("write"))return 1;
    char path[4096];
    return !fcntl(fd,F_GETPATH,path) && strstr(path,"/values/") && fault("pixel_write");
}
static int tr_open(const char *p,int flags,...) {
    mode_t m=0; if(flags&O_CREAT){va_list ap;va_start(ap,flags);m=va_arg(ap,int);va_end(ap);}
    double t=now();int fd=open(p,flags,m);record(kind(fd),0,fd,now()-t,!!(flags&O_CREAT));return fd;
} INTERPOSE(tr_open,open)
static int tr_openat(int d,const char *p,int flags,...) {
    mode_t m=0;if(flags&O_CREAT){va_list ap;va_start(ap,flags);m=va_arg(ap,int);va_end(ap);}
    double t=now();int fd=openat(d,p,flags,m);record(kind(fd),0,fd,now()-t,!!(flags&O_CREAT));return fd;
} INTERPOSE(tr_openat,openat)
static ssize_t tr_write(int fd,const void *b,size_t n) {
    int k=kind(fd);double t=now();ssize_t r=write_fault(fd,k)?-1:write(fd,b,n);record(k,1,r,now()-t,0);return r;
} INTERPOSE(tr_write,write)
static ssize_t tr_pwrite(int fd,const void *b,size_t n,off_t off) {
    int k=kind(fd);double t=now();ssize_t r=write_fault(fd,k)?-1:pwrite(fd,b,n,off);record(k,1,r,now()-t,0);return r;
} INTERPOSE(tr_pwrite,pwrite)
static ssize_t tr_writev(int fd,const struct iovec *b,int n) {
    int k=kind(fd);double t=now();ssize_t r=write_fault(fd,k)?-1:writev(fd,b,n);record(k,1,r,now()-t,0);return r;
} INTERPOSE(tr_writev,writev)
static ssize_t tr_read(int fd,void *b,size_t n) {
    int k=kind(fd);double t=now();ssize_t r=read(fd,b,n);record(k,2,r,now()-t,0);return r;
} INTERPOSE(tr_read,read)
static ssize_t tr_pread(int fd,void *b,size_t n,off_t off) {
    int k=kind(fd);double t=now();ssize_t r=pread(fd,b,n,off);record(k,2,r,now()-t,0);return r;
} INTERPOSE(tr_pread,pread)
static int sync_fault(int fd,int full) {
    char p[4096];if(fcntl(fd,F_GETPATH,p))return 0;
    int k=kind(fd);
    if(k==3)return fault(strstr(p,".partial-")?"directory_sync":(committed?"parent_sync":"ancestor_sync"));
    if(full && k==2)return fault(committed?"post_sync":"final_sync");
    if(k>0 && strstr(p,"/values/") && fault("pixel_sync"))return 1;
    return k>0 ? fault("file_sync") : 0;
}
static int tr_fsync(int fd) {
    int k=kind(fd);double t=now();int r=sync_fault(fd,0)?-1:fsync(fd);record(k,3,r,now()-t,0);return r;
} INTERPOSE(tr_fsync,fsync)
static int tr_fcntl(int fd,int cmd,...) {
    long arg=0;va_list ap;va_start(ap,cmd);
    switch(cmd) {
        case F_GETFD:case F_GETFL:case F_GETPATH:case F_FULLFSYNC:
            if(cmd==F_GETPATH)arg=(long)va_arg(ap,void*);break;
        case F_SETFD:case F_SETFL:case F_NOCACHE:arg=va_arg(ap,int);break;
        default:arg=(long)va_arg(ap,void*);break;
    }
    va_end(ap);if(cmd!=F_FULLFSYNC)return fcntl(fd,cmd,arg);
    int k=kind(fd);double t=now();int r=sync_fault(fd,1)?-1:fcntl(fd,cmd,arg);record(k,4,r,now()-t,0);return r;
} INTERPOSE(tr_fcntl,fcntl)
static int tr_mkdir(const char *p,mode_t mode) {
    double t=now();int r=mkdir(p,mode);double elapsed=now()-t;
    if(path_kind(p)>0){pthread_mutex_lock(&lock);mkdirs++;mkdir_s+=elapsed;pthread_mutex_unlock(&lock);}return r;
} INTERPOSE(tr_mkdir,mkdir)
static int tr_renamex_np(const char *a,const char *b,unsigned int flags) {
    if(path_kind(a)<=0)return renamex_np(a,b,flags);
    double t=now();int r=fault("rename")?-1:renamex_np(a,b,flags);double elapsed=now()-t;
    pthread_mutex_lock(&lock);renames++;rename_s+=elapsed;if(!r)committed=1;pthread_mutex_unlock(&lock);return r;
} INTERPOSE(tr_renamex_np,renamex_np)
__attribute__((destructor)) static void finish(void) {
    const char *path=getenv("TR_IO_REPORT");if(!path)return;FILE *f=fopen(path,"w");if(!f)return;
    fprintf(f,"{\"mkdirs\":%lu,\"mkdir_seconds\":%.9f,\"renames\":%lu,\"rename_seconds\":%.9f",mkdirs,mkdir_s,renames,rename_s);
    for(int i=0;i<4;i++) {
        Stats *s=&stats[i];fprintf(f,",\"%s\":{\"opens\":%lu,\"creates\":%lu,\"writes\":%lu,\"write_bytes\":%lu,\"reads\":%lu,\"read_bytes\":%lu,\"fsyncs\":%lu,\"fullsyncs\":%lu,\"errors\":%lu,\"open_seconds\":%.9f,\"write_seconds\":%.9f,\"read_seconds\":%.9f,\"fsync_seconds\":%.9f,\"fullsync_seconds\":%.9f}",
            (const char*[]){"source","output","manifest","directories"}[i],s->opens,s->creates,s->writes,s->bytes,s->reads,s->read_bytes,s->fsyncs,s->fullsyncs,s->errors,s->open_s,s->write_s,s->read_s,s->fsync_s,s->fullsync_s);
    }
    fprintf(f,"}\n");fclose(f);
}
