/* macOS benchmark-only interposer. Counts successful kernel-facing bytes, not
 * physical device traffic. No behavior changes: fsync/close errors propagate.
 * Build: cc -O2 -dynamiclib bench/io_profile.c -o prototype-output/write-profile/io.dylib
 */
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/stat.h>
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
typedef struct {unsigned long opens,creates,reads,read_bytes,writes,write_bytes,syncs,closes; double open_s,read_s,write_s,sync_s,close_s;} Stats;
static Stats stats[3];
static pthread_mutex_t lock=PTHREAD_MUTEX_INITIALIZER;
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static int kind(int fd){int saved=errno;char path[4096];int result=-1;if(fd>=0 && !fcntl(fd,F_GETPATH,path)){const char *o=getenv("TR_IO_OUTPUT"),*s=getenv("TR_IO_SOURCE");if(o&&strncmp(path,o,strlen(o))==0){result=1;const char*base=strrchr(path,'/');if(base&&(!strcmp(base+1,"manifest.json")||!strcmp(base+1,".manifest.tmp")))result=2;}else if(s&&strncmp(path,s,strlen(s))==0)result=0;}errno=saved;return result;}
static void record(int k,int op,ssize_t n,double sec,int create){if(k<0)return;pthread_mutex_lock(&lock);Stats *s=&stats[k];switch(op){case 0:s->opens++;s->creates+=create;s->open_s+=sec;break;case 1:s->reads++;s->read_bytes+=n>0?n:0;s->read_s+=sec;break;case 2:s->writes++;s->write_bytes+=n>0?n:0;s->write_s+=sec;break;case 3:s->syncs++;s->sync_s+=sec;break;case 4:s->closes++;s->close_s+=sec;break;}pthread_mutex_unlock(&lock);}
static int tr_open(const char *p,int flags,...){mode_t m=0;if(flags&O_CREAT){va_list ap;va_start(ap,flags);m=va_arg(ap,int);va_end(ap);}double t=now();int fd=open(p,flags,m);double d=now()-t;int k=kind(fd);if(k==0 && getenv("TR_IO_NOCACHE") && !strcmp(getenv("TR_IO_NOCACHE"),"1"))fcntl(fd,F_NOCACHE,1);record(k,0,0,d,!!(flags&O_CREAT));return fd;} INTERPOSE(tr_open,open)
static int tr_openat(int dir,const char *p,int flags,...){mode_t m=0;if(flags&O_CREAT){va_list ap;va_start(ap,flags);m=va_arg(ap,int);va_end(ap);}double t=now();int fd=openat(dir,p,flags,m);double d=now()-t;int k=kind(fd);if(k==0 && getenv("TR_IO_NOCACHE") && !strcmp(getenv("TR_IO_NOCACHE"),"1"))fcntl(fd,F_NOCACHE,1);record(k,0,0,d,!!(flags&O_CREAT));return fd;} INTERPOSE(tr_openat,openat)
static ssize_t tr_write(int fd,const void*b,size_t n){int k=kind(fd);double t=now();ssize_t r=write(fd,b,n);record(k,2,r,now()-t,0);return r;} INTERPOSE(tr_write,write)
static ssize_t tr_pwrite(int fd,const void*b,size_t n,off_t off){int k=kind(fd);double t=now();ssize_t r=pwrite(fd,b,n,off);record(k,2,r,now()-t,0);return r;} INTERPOSE(tr_pwrite,pwrite)
static ssize_t tr_writev(int fd,const struct iovec*b,int n){int k=kind(fd);double t=now();ssize_t r=writev(fd,b,n);record(k,2,r,now()-t,0);return r;} INTERPOSE(tr_writev,writev)
static ssize_t tr_read(int fd,void*b,size_t n){int k=kind(fd);double t=now();ssize_t r=read(fd,b,n);record(k,1,r,now()-t,0);return r;} INTERPOSE(tr_read,read)
static ssize_t tr_pread(int fd,void*b,size_t n,off_t off){int k=kind(fd);double t=now();ssize_t r=pread(fd,b,n,off);record(k,1,r,now()-t,0);return r;} INTERPOSE(tr_pread,pread)
static int tr_fsync(int fd){int k=kind(fd);double t=now();int r=fsync(fd);record(k,3,0,now()-t,0);return r;} INTERPOSE(tr_fsync,fsync)
static int tr_close(int fd){int k=kind(fd);double t=now();int r=close(fd);record(k,4,0,now()-t,0);return r;} INTERPOSE(tr_close,close)
__attribute__((destructor)) static void finish(void){const char *p=getenv("TR_IO_REPORT");if(!p)return;char report[4096];snprintf(report,sizeof(report),"%s.%d",p,getpid());FILE*f=fopen(report,"w");if(!f)return;fprintf(f,"{");for(int i=0;i<3;i++){Stats*s=&stats[i];fprintf(f,"%s\"%s\":{\"opens\":%lu,\"create_opens\":%lu,\"reads\":%lu,\"read_bytes\":%lu,\"writes\":%lu,\"write_bytes\":%lu,\"syncs\":%lu,\"closes\":%lu,\"open_seconds\":%.9f,\"read_seconds\":%.9f,\"write_seconds\":%.9f,\"sync_seconds\":%.9f,\"close_seconds\":%.9f}",i?",":"",i==2?"manifest":(i?"output":"source"),s->opens,s->creates,s->reads,s->read_bytes,s->writes,s->write_bytes,s->syncs,s->closes,s->open_s,s->read_s,s->write_s,s->sync_s,s->close_s);}fprintf(f,"}\n");fclose(f);}
/* Rust File::sync_all uses F_FULLFSYNC on Darwin, falling back to fsync. */
static int tr_fcntl(int fd,int cmd,...){long arg=0;va_list ap;va_start(ap,cmd);switch(cmd){case F_GETFD:case F_GETFL:case F_FULLFSYNC:break;case F_SETFD:case F_SETFL:case F_NOCACHE:arg=va_arg(ap,int);break;default:arg=(long)va_arg(ap,void*);break;}va_end(ap);if(cmd!=F_FULLFSYNC)return fcntl(fd,cmd,arg);int k=kind(fd);double t=now();int r=fcntl(fd,cmd,arg);record(k,3,0,now()-t,0);return r;} INTERPOSE(tr_fcntl,fcntl)
