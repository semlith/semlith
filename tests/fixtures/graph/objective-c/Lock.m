#import <Foundation/Foundation.h>

#define MAX_HOLDERS 4

@implementation Lock

- (int)helper { return 1; }

- (int)acquire { return [self helper]; }

@end
