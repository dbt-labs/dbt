# This file contains functions that the rust module imports.
from . import TemplateError, safe


def make_error(info):
    # Internal utility function used by the rust binding to create a template error
    # with info object.  We cannot directly create an error on the Rust side because
    # we want to subclass the runtime error, but on the limited abi it's not possible
    # to create subclasses (yet!)
    err = TemplateError(info.description)
    err._info = info
    return err


# used by the rust runtime to mark something as safe
mark_safe = safe
