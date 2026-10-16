// Cut down from cobra command.go: the builtin and standard-library calls that
// filled the 2026-10-14 audit's "Called and not found" list (item 30), beside
// one call to code the corpus really does not hold.
package cobra

import (
	"fmt"
	"os"
	"strings"

	flag "github.com/spf13/pflag"
)

// Command is just that, a command for your application.
type Command struct {
	Use    string
	groups []string
	flags  *flag.FlagSet
}

// checkCommandGroups checks if a command has been added to a group that does not exist.
func (c *Command) checkCommandGroups() error {
	if len(c.groups) == 0 {
		return fmt.Errorf("group id is not defined for subcommand '%s'", c.Use)
	}
	c.groups = append(c.groups, strings.TrimSpace(c.Use))
	fmt.Fprintln(os.Stderr, "checked")
	return c.validateRequiredFlags()
}
