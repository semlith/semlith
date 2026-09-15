#import <Foundation/Foundation.h>

@implementation Lock

- (int)helper { return 1; }

- (int)acquire { return [self helper]; }

@end
