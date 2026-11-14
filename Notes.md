## Settings

- Setting page needs to be cleaned up. Ideally add sections or dropdowns. Different pages would be cleaner.
- API Keys box needs to be wider, probably add a button to show/hide the keys when they are typed in.
- For the download folders, it would be good if we can add multiple folders to watch not just the default.

## Search
- Would be good to see the arrows to show you can sort by each column
- Price (native) needs to show which currency is being displayed for each
- Need to improve the Get button, it has ... which looks like there is more to the button and it would be good if it was clearer to the user what it actually is.
- The favourites button also is unclear and am not sure if it is even needed here.
- Assuming the @1 is the price for 1 unit, would be good if this was clear, might also be worth considering if we can say if its 1 unit or a pack, some parts come in quanities.
- Would be good to expand so we can search by part type ( say I am looking for a switch or MCU), would also be good to show an image.
- For get it would also be good to show what is avilable (schematic, footprint, 3D, simulation datasheet eta)

## Import
- It would be good if the import zip could also import the different types of files.
- For import library, I tried an Altium file and a Fusion and neither worked. 

## Library
- I think we should consider how edit, preview, Attach, copy to project and open datasheet are displayed. Its currently kind of messy. 
- It would be good to also be able to sort and filter on this page.
- I deleted a datasheet, the link was broken but download missing datasheet didn't go and find the datasheet it was just looking for a link. Ideally this feature is smart enough to go and find the actual datahseet and download it.

## History 
- Might have to be smart about how we implement this. Possibly allowing the user to set a major version and small changes are minors. Allows use to reduce the overall size of the history. 

# Wave 3
## Standardise to KLC
- Originally I was thinking also about the scheamtic, so all of resistors, caps, ICs, eta where all in the same style and layout so you can be consisent. Regardless of the imported components.

## Alternatives
- This might still need some work, but it also only looked in the library, ideally you can also search and define alternate parts and import them.


# KiCAD
- Firstly we should be updating the versions for each time, so we should be at least up to 0.2.0 or go of the wave.
- The current version doesn't open and it would be good to have it in the schematic view as well.
- A feature feature would be to allow the tool to also clean up all avilable librarys (symbols, footprints eta) that are avilable within KiCAD. The user could then chose to remove, fix or integrate the existing librabies into the KLM.
- A number of changes require KiCAD to be closed before they can work. It would be good if we can make as main of them be able to work correctly with KiCAD still open. 


# Current version (10:50 - 13/11/26)
- Look up lines button needs to be more intuitve to what it is actually for. Possibly a serpare or the "same" button as generate as it is required before you can generate.
- Should only look up selected lines, have an edit button/find (Symbol / footprint / 3D). Also need to standardise with the rest of the app
- Likely Import can be moved into either search or Library.
- Need to fix rate limit
- Import option from search, would be good to include more infromation on the part
- Add indicate to say if in library already
- If something exists in the library, by default don't reimport, maybe ask the user to confirm they want to reimport. Should add loading or progress bar for import
- Improve performance, footprint on import (from search) takes to long to display
- reimport button in library
- Updates in KiCAD come back into KLM
- 