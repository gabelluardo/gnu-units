#include "units.h"
#include <stdio.h>

extern char *mylocale;
extern char *progname;
extern int utf8mode;

int completereduce(struct unittype *unit);

int newunit(char *unitname, char *unitdef, int *count, int linenum,
            char *file, FILE *errfile, int redefine, int userunit);

int newprefix(char *unitname, char *unitdef, int *count, int linenum,
              char *file, FILE *errfile, int redefine);

int newtable(char *unitname, char *unitdef, int *count, int linenum,
             char *file, FILE *errfile, int redefine);

int newfunction(char *unitname, char *unitdef, int *count, int linenum,
                char *file, FILE *errfile, int redefine);

int newalias(char *unitname, char *unitdef, int linenum, char *file,
             FILE *errfile);
