bcReliable PDF Creation in the Enterprise Dov Isaacs Principal Scientist, Product Interoperability Adobe Systems Incorporated November 5, 2001 PDF 2001 Conference West

bc 2 Agenda Introduction Content Creation PDF File Creation Post-PDF File Creation Tweaking & "Other" Considerations Q&A

bc Introduction

bc 4 Introduction The Enterprise We are not talking about Star Trek In our context, enterprise refers to an organization conducting day-to-day business which is not primarily creative content creation, prepress service, or publishing May include governmental agencies, academia, and R&D organizations

bc 5 Introduction The Enterprise Organizations in which PDF can be created and used as a content communications media for display and on-demand printing with occasional external production needs Creators and recipients of PDF in such organizations are primarily Windows-based (90%+) but must maintain compatibility with the 10% who "think different"

bc 6 Introduction The Enterprise Typical applications from which PDF is derived: Microsoft Office Corel WordPerfect Lotus WordPro Adobe FrameMaker Microsoft Internet Explorer FileMaker Pro Microsoft Visio, Project, and Publisher JASC Paint Shop Pro CorelDRAW Specialized & industry-specific application programs

bc 7 Introduction The Enterprise And some content from professional graphics and document layout programs: Adobe Illustrator Adobe Photoshop Macromedia Freehand Adobe PageMaker Quark XPress Adobe InDesign

bc 8 This is NOT... A sales pitch (if you are attending this conference, you have already bought into Acrobat and PDF) A tutorial on Advanced Techniques (other sessions deal with the specifics of hyperlinks, indexing, multimedia, forms, collaboration, prepress, etc.) Rocket Science

bc 9 We will... Solve the puzzle as to how to reliably, consistently, and easily create PDF files without gurus and prepress experts --- mere mortals can do this!

bc 10 We will... Discuss: Issues associated with content creation Limiting the number of settings, options files, and decisions necessary to create a PDF file Techniques and shortcuts to create lean, mean, high quality PDF files suitable for: Display (and web) Printing & "Low end" prepress Debunk myths and urban legends surrounding basic issues of PDF creation

bc 11 Dov Isaacs agrees with Ron Popeil... "Set it and Forget it"

bc 12 The Fine Print (pun intended)... The material presented today may challenge long-held religious beliefs about how to create content, PostScript, and PDF The opinions and techniques presented are those of the presenter, Dov Isaacs, and do not necessarily represent opinions held by or techniques officially endorsed by Adobe Systems Incorporated You mileage may vary, but DO try this at home!

bc Content Creation

bc 14 Content Creation General Considerations A PDF file can never be better than the content from which it is created GIGO: Garbage in, garbage out!

bc 15 Content Creation General Considerations $2,500 buys a tremendous amount of computer, printer, and software, well beyond even the dreams of publishing professionals fifteen years ago Today, we can very rapidly and inexpensively produce cruddy, shlocky-looking junk There is just no accounting for bad taste

bc 16 Content Creation General Considerations We will discuss the mechanics of content creation, not issues of taste and style! 345 Mishagoss Lane West Pork Chop, CA 95075 (408) 936-1212 hunckledinkle@mumble.org Gustav Hunckledinkle Objective I wanna gud prophphesional posishun! Experience 2000 -- 2001 Glurbish Design Studio West Pork Chop, CA Grafic Designer Helped rite and layout many documents & pamflets. Layed off when customers went elsewhere. 1985 -- 2000 Acme Supply Company East Pork Chop, CA Quality Manager Personally inspected all stuphph shipped to the coyote. Education 1985 General Custer High Scool Surrender, WY Graduated Summa Cum Lowly. Interests Fast cars and booze.

bc 17 Content Creation General Considerations Maintain content at its highest level of abstraction by category and within category; expedience can byte you terribly later! Text (as realized via fonts) trumps Vector Graphics (lines & polygons) & Vector Graphics (lines & polygons) trump Images (raster data)

bc 18 Content Creation General Considerations Lose no data before its time...... add no unnecessary data Image data Transparency information Color information Avoid unnecessary and cascading data and attribute transformations Lossy compressions Colorspace transformations Artifacts from flattening of transparency

bc 19 Text Keep text as "text" Avoid "convert to outline" or "rasterize" operations except for complex, very special artistic effects Why? Text rendered with fonts has smart scalability due to "hinting" of mathematical character shape definitions Displays and prints with much higher quality, especially at smaller point sizes and on low resolution devices Vector artwork is not "hinted" as are commercial Type 1, TrueType, and OpenType fonts -- loss of readability when scaled down Images are resolution dependent and not readily scalable without serious quality loss PDF searchability and touch-up capability for text is lost with conversions to vector graphics or images

bc 20 Text Fonts Adobe PostScript and PDF both support Type 1 and TrueType fonts natively and equally well (Yes, this is contrary to what many so-called graphics and prepress professionals may continue to tell you!) Choice of fonts should be based on: Aesthetics Appropriateness of font design to its proposed use Quality of font production including adherence to standards (such as encoding and character sets) Licensing terms -- font vendor must allow embedding of fonts in PostScript and PDF for purposes of "preview and print" -- otherwise, the font license is effectively USELESS!

bc 21 Text Fonts (continued) Avoid "hacked fonts" Font tools such as Fontographer and FontLab Great for creating and editing new fonts NOT lossless editors and "converters" of existing fonts Changes to metrics may alter line and page breaks Possible changes to design (bezier versus quadratic) Loss of kerning data Loss of "hinting" data degrades display/print quality For custom characters and logos Do not add or substitute characters into existing fonts Commission special symbol font from type foundry NEVER, repeat NEVER modify a commercial font and resave it with the same name!

bc 22 Text Fonts (continued) For non-Western Latin character sets Do not use fonts that masquerade as Western Latin fonts Results in data conversion problems later Problems with text touch-up and search in Acrobat Use applications that support Unicode Use OpenType fonts with support for desired character sets " 1) " (: -,' -.,. " - "; ' " ' " -, : 1 (. 2 (.,,.,,. -,, " ']" "; ".[: : " ' 4a - -- « » ?,, ?,.,.,.. (" ") ("a ") -,. : "." "." " " " " 1996. " " " " Each of these samples is composed in "Arial"

bc 23 Text Fonts (continued) Avoid "amateur hour" font production like the plague! Existing tools make it easy to create a font file Extensive knowledge and experience are required to use these tools to produce quality fonts that interface properly with: Latest versions of Windows and Mac operating systems Application programs that directly control fonts RIPs Acrobat and ability to be embedded in PDF files What looks OK on-screen may fail or look terrible farther down in the workflow

bc 24 Vector Graphics Vector Graphics graphical objects represented by stroked and filled polygons and stroked line segments For non-text artwork, offers highest flexibility No data loss under transformations: Scaling Rotation Masking Minimal graphic display quality degradation when transformed (primarily when downscaling)

bc 25 Vector Graphics Content creation and editing via "draw" programs, not "paint" programs Adobe Illustrator Macromedia Freehand CorelDRAW Microsoft Visio "Paint" programs allow creation of stroked and filled polygons and stroked line segments, but such objects are exported and output strictly as image data

bc 26 Vector Graphics Investment required to learn to use tools properly and effectively Rectangle as a "rectangle" as opposed to four nearly- touching line segments Difficulty in visualizing shapes, widths, color, and effects as they will appear as ultimately used and either displayed or printed Low resolution screens versus high resolution output devices can result in misjudgment of correct line widths RGB screen display (high gamut) versus CMYK print output (lower gamut)

bc 27 Vector Graphics Not all output is what you expect it to be! Examples: Gradient fills bunches of polygons of different colors or device resolution image data Vector effects such as drop shadows device resolution image data Text filled outlines All objects device resolution image data Causes: Inherent problems due to file format or file format version PostScript 3 versus PostScript Level 2 or Level 1 TIFF, GIF, JPEG are image-only formats Wrong export / save options specified Inherent "limitations" of content creation program

bc 28 Vector Graphics Specific recommendations Save vector graphics-based artwork in the highest level file format your workflow can "digest" PDF 1.4 for import into InDesign 2 (1.3 for earlier versions) EPS with PostScript 3 and fonts embedded for other applications Transparency (Illustrator 9 and 10) Works best with InDesign 2 and PDF 1.4 Otherwise, requires implicit flattening Can cause objects in region of transparency to be decomposed, text converted to vector graphics, and/or vector graphics converted to images For flattening options, choose "highest quality / slowest speed" and appropriate image resolution

bc 29 Vector Graphics Specific recommendations (continued) Gradients, blends, fountain fills Choose object type carefully (Illustrator - blends never generate PostScript 3 / PDF 1.3 smooth-shaded gradients, gradients do) Choose export format carefully PDF 1.3 and above retain smooth-shaded gradients PostScript 3 / EPS with language level 3 required to retain smooth-shaded gradients All other formats decompose gradients Be careful in defining colors Spot color versus named composite color definitions Spot colors should be defined and used only when you really need them

bc 30 Vector Graphics Specific recommendations (continued) Beware of vector artwork from CAD programs AutoCAD and others output PostScript of the form 0 setlinewidth resulting in stroked lines that are minimal width renderable, i.e., one pixel in width, regardless of device Workarounds: Import and edit in "draw" program before importing into target document Download and install Prinergy Distiller Plug-in from CreoScitex at: <http://www.creo.com/prinergy/distillerplugindownload2.asp>

bc 31 Vector Graphics And finally... Most of us are NOT gifted, graphic artists Good vector-based clip art is well worth its cost; it is much better than sloppy, poorly-rendered, amateur-hour graphics!

bc 32 Images Images graphical objects represented by raster data The most flexible and the least flexible format All graphics can be and ultimately are represented in image format -- "edit" can be at the pixel level Lowest level of abstraction for graphics Potential data loss and graphic display quality degradation under transformations: Scaling Downsampling -- data loss Interpolation -- quality loss Rotations (usually at other than 90° increments)

bc 33 Images Content creation via "paint" programs Adobe Photoshop (and Photoshop Elements) Corel PhotoPaint JASC Paint Shop Pro Images necessary for Computer screen shots Reproduction of photographs Artwork that cannot otherwise be readily represented as text or vector graphics Investment required to learn to use tools properly and effectively

bc 34 Images Specific Recommendations Scan photographic images at reasonable resolutions Scanner support for 2400 dpi doesn't mean that an 8"x10" photo should be scanned at 2400 dpi unless significant magnification and cropping is involved Consider downsampling before saving image Screen shots (and other image graphics) Do NOT interpolate to higher resolutions Extra image data is bloat carried through the workflow and into the PDF file No quality improvement achieved; can result in poorer image display and printing Use "image interpolation" option when saving from Photoshop as EPS or PDF or use "cheap Prologue.ps trick" discussed later!

bc 35 Images Specific recommendations (continued) Save as EPS format image if exact color must be passed through to Distiller Cardinal rule: Postpone image transformations as much as possible to later phases in the workflow Avoid lossy data compression (such as JPEG for photographic images) until creation of final PDF file Avoid color transformations until display or print time (more about color later!) Remember that some formats such as GIF are inherently lossly due to 8-bit, indexed color

bc PDF File Creation

bc 37 PDF File Creation PDF Files may be created by three methods: Application Direct PDF Export PDF via Distillation of PostScript PDF via PDFWriter

bc 38 PDF File Creation Application Direct PDF Export Content Direct PDF Export Capable Applications PDF File

bc 39 Content PDF File PDF File Creation PDF File Creation via Distillation of PostScript Create Adobe PDF (MacOS) Acrobat Distiller (Windows) PostScript File Acrobat Distiller Distiller Job Options Files PostScript Driver All Applications

bc 40 All Applications Content PDF 1.2 File PDF File Creation PDF File Creation via PDFWriter Driver PDFWriter (MacOS) PDFWriter (Windows) Driver

bc 41 PDF File Creation Application Direct PDF Export trumps PDF via Distillation of PostScript Applications with the capability of directly generating PDF from all content: Adobe InDesign Adobe Illustrator Adobe Photoshop Others? Efficiency -- Single pass operation Support for PDF objects and attributes that have no corresponding feature in PostScript (example, transparency)

bc 42 PDF File Creation PDF via Distillation of PostScript trumps PDF via PDFWriter Support of EPS graphics requires PostScript output stream and subsequent distillation to create PDF Application, driver, and OS support for PostScript: Highest level of graphics support for most applications Most mature driver Globally-recognized "escape" mechanism by which PDFMark can supplement standard PostScript for PDF creation (PDFMaker as well as third party products for FrameMaker) PDFWriter support ends with PDF 1.2; default installation of Acrobat 5.0.x does not install PDFWriter

bc 43 PDF File Creation via Distillation of PostScript Generate PostScript per capabilities of PDF, not the capabilities of "final" target print device Acrobat Distiller PPD in conjunction with the PostScript driver PostScript Language Level 3 Native TrueType support Acrobat Distiller (Windows) & Create Adobe PDF (Mac) printer driver instances Created automatically by Acrobat Installer with correct PPD file Obviates most any need to manually create and distill PostScript or to maintain the PostScript files

bc 44 PDF File Creation via Distillation of PostScript However, some of the "as installed" default driver settings could use some "tweaking" for best (or even usable) results...

bc 45 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Controls when and how "printing" occurs Change spool options

bc 46 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Spooling dramatically improves "return control to application" time Printing after last page is spooled avoids possible PostScript timeout problems in Distiller On the Macintosh, use Background Printing

bc 47 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Device Settings Controls device specific parameters

bc 48 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Device Settings Output Protocol Binary yields less and more efficient PostScript for distillation CTRL-D is technically incorrect for binary channels (although Distiller kludges around it) Wait Timeout = 0 avoids Distiller or PostScript printer job cancellation due to transient system or network slowdown

bc 49 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Device Settings Convert Gray Text/Graphics to PostScript Gray yields more efficient PostScript and PDF Windows GDI is totally RGB; application black and grayscale expressed as R=G=B Black and grayscale as RGB can lead to "rich black" (C=M=Y or C=M=Y=K) printing or prepress problems Option enabled causes driver to change all GDI text and line art graphics (not images) for which R=G=B to equivalent K (grayscale) Postscript Macintosh QuickDraw supports CMYK; no need for comparable driver options

bc 50 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Layout Advanced & Properties General Printing Preferences Layout Advanced Controls additional document-oriented PostScript generation options

bc 51 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Layout Advanced & Properties General Printing Preferences Layout Advanced 600DPI? Windows2000/XP/NT4Bug For GDI applications only, "large text" is passed to driver as filled outlines -- poor rendering / no searchability and PDF touch-up Crossover point dependent upon point size, specified print device resolution, font, and Windows version 600 DPI setting under Windows 2000 / XP typically allows for 144 pt text, 1200 DPI setting doesn't Minor but endurable side effects

bc 52 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Layout Advanced & Properties General Printing Preferences Layout Advanced TrueType Font Download as Softfont Option avoids any substitution of host-based TrueType font with "printer resident" font Problems with substitution: Differences in style Differences in character sets supported Less of a problem with the Acrobat Distiller printer instance; potentially a massive problem with "real" PostScript printers with documents using international character sets

bc 53 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Layout Advanced & Properties General Printing Preferences Layout Advanced TrueType Font Download Option Native TrueType Explicit setting of option avoids surprises Conversion of TrueType fonts yields degraded results Bitmaps are at device resolution / not scalable or searchable Outlines are "unhinted" Type 1 fonts No PDF touch-up for text in these fonts

bc 54 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Layout Advanced & Properties General Printing Preferences Layout Advanced PostScript Language Level 3 Should always be specified for the Distiller, regardless of the capabilities of the final printing device Acrobat / Acrobat Reader printing capable of printing all constructs to PostScript printers of all language levels

bc 55 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Adobe PDF Settings & Properties General Printing Preferences Adobe PDF Settings Controls the interface between the PostScript driver and the "background" Distiller plus specification of distillation job options

bc 56 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Adobe PDF Settings & Properties General Printing Preferences Adobe PDF Settings Conversion settings Specify the appropriate job options for distillation Defaults for printer, once set, can be changed or overridden on a job-by-job basis Live dangerously; create and use your own job options appropriate to your needs Ask to Replace existing PDF file option prevents accidental overwrite of existing PDF file with the same name

bc 57 PostScript Driver Setup PDF Production & Otherwise Windows 2000 / XP -- Properties Advanced Printing Defaults Adobe PDF Settings & Properties General Printing Preferences Adobe PDF Settings Do not send fonts to Distiller -- Huh? Very counter-intuitive to what we've been preaching Distiller now find and embeds Type 1, TrueType, and OpenType fonts from system & user locations Why? Efficient and compact intermediate PostScript Faster distillation and less chance of VM problems Better and more consistent embedding of font subsets Exceptions? Private, application-installable fonts

bc 58 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- Print PDF Settings Controls the interface between the PostScript driver and the "background" Distiller plus specification of distillation job options

bc 59 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- Print PDF Settings Conversion settings Specify the appropriate job options for distillation Live dangerously; create and use your own job options appropriate to your needs After PDF Creation option allows preview of the PDF file Unlike Windows, default is Launch Nothing Reasonable option is to Launch Adobe Acrobat Save Settings allows "stickiness" of settings by application program

bc 60 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- Print PostScript Settings Determines type and format of PostScript generated plus options for font embedding in the PostScript stream

bc 61 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- Print PostScript Settings PostScript Level -- Level 3 Only Should always be specified for the Distiller, regardless of the capabilities of the final printing device Acrobat / Acrobat Reader printing capable of printing all constructs to PostScript printers of all language levels Data Format Binary yields less and more efficient PostScript for distillation

bc 62 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- Print PostScript Settings Do not send fonts to Distiller -- Again! Same issues and reasoning as under Windows Works around TrueType font embedding bug on Mac: All TrueType fonts embedded in PostScript stream by MacOS end up as "unhinted" Type 1 outlines Quality degradation PDF text touch-up problems MacOS (PrintingLib 8.7.x module)!! Exceptions? Private, application-installable fonts Save Settings (as previously described)

bc 63 PostScript Driver Setup PDF Production & Otherwise Mac OS 8.x and 9.x -- How to Drive a Mac User Crazy! PDF file fully produced under MacOS 9.1 Encoding names are historical PDF files with fonts "Windows-encoded" display, print, and touch-up without any problems on Macintosh (and vice-versa)!

bc 64 Distiller Setup Preferences Provides output options for when Distiller is invoked "manually" Delete Log Files for successful jobs option for all invocations of Distiller You've been asking for this feature for years! Turn off only for debugging purposes

bc 65 Distiller Job Options Acrobat Distiller provides four sets of predefined job options: Screen, eBook, Print, and Press Job options based on intended usage and audience; Screen Press: Low resolution High resolution Medium quality, highly compressed images Maximum quality, less compressed images Less emphasis on typographical fidelity Perfect typographical fidelity RGB, screen-oriented managed color Use color exactly as specified in PostScript file Smaller PDF files Larger PDF files

bc 66 Distiller Job Options The predefined job options attempt to optimize PDF files by intended usage Yields scenarios in which "view and print anywhere" with a single PDF file is not quite true! Assumes that: Screen display PDF doesn't require high quality typography Image quality loss for screen display PDF files is acceptable Print or press quality PDF is necessarily too big or otherwise unacceptable for screen display purposes Operating system / driver / PostScript-based color management actually works and is properly invoked by enterprise application programs and users Poor quality printed output from screen display PDF is OK Repurposing of existing PDF is not important

bc 67 Distiller Job Options You can do better using fewer sets of customized job options in conjunction with more carefully and consistently-prepared content!

bc 68 Distiller Job Options Compared Standard Distiller & Isaacs Job Options Sets Left Left Left Left Left Left Binding Automatic, Maximum Quality Bicubic 150 dpi, images over 225 Automatic, Maximum Quality Bicubic 150 dpi, images over 225 612 x 792 pts 2400 dpi Individually PDF 1.4 Isaacs 150 Automatic, Medium Quality Average 72 dpi, images over 108 Automatic, Medium Quality Average 72 dpi, images over 108 612 x 792 pts 600 dpi Individually PDF 1.2 Screen PDF 1.4 PDF 1.3 PDF 1.3 PDF 1.3 Compatibility Automatic, Maximum Quality Automatic, Maximum Quality Automatic, High Quality Automatic, Medium Quality Grayscale Image Compression Bicubic 300 dpi, images over 450 Bicubic 300 dpi, images over 450 Bicubic 300 dpi, images over 450 Bicubic 150 dpi, images over 225 Grayscale Image Downsampling Automatic, Maximum Quality Automatic, Maximum Quality Automatic, High Quality Automatic, Medium Quality Color Image Compression Bicubic 300 dpi, images over 450 Bicubic 300 dpi, images over 450 Bicubic 300 dpi, images over 450 Bicubic 150 dpi, images over 225 Color Image Downsampling 612 x 792 pts 612 x 792 pts 612 x 792 pts 612 x 792 pts Default Page Size 2400 dpi 2400 dpi 1200 dpi 600 dpi Resolution Individually Collectively Auto-Rotate Pages Embed Thumbnails Optimize for Fast Web View Isaacs Press Print eBook

bc 69 Distiller Job Options Compared Standard Distiller & Isaacs Job Options Sets Cancel Job Cancel Job Cancel Job Warn & Continue Warn & Continue Warn & Continue If Embedding Fails Bicubic 1200 dpi, images over 1800 Bicubic 300 dpi, images over 450 Bicubic 1200 dpi, images over 1800 Bicubic 1200 dpi, images over 1800 Bicubic 300 dpi, images over 450 Average 300 dpi, images over 450 Monochrome Image Downsampling CCITT Group 4 CCITT Group 4 CCITT Group 4 CCITT Group 4 CCITT Group 4 CCITT Group 4 Monochrome Image Compression < 100% of characters used Isaacs 150 Times, Courier, Helvetica, Symbol, & Zapf Dingbats < 100% of characters used Screen Monochrome Image AntiAliasing Times, Courier, Helvetica, Symbol, & Zapf Dingbats Never Embed Always Embed < 100% of characters used < 100% of characters used < 100% of characters used < 100% of characters used Subset Embedded Fonts Embed ALL Fonts Compress Text & Line Art Isaacs Press Print eBook

bc 70 Distiller Job Options Compared Standard Distiller & Isaacs Job Options Sets Preserve Leave Color Unchanged; Default Intent Isaacs 150 Preserve US Web Coated (SWOP) v2 sRGB IEC61966-2.1 Convert Everything to CalRGB; Default Intent Screen Color Settings File Use Prologue.ps & Epilogue.ps Preserve Halftone Information Preserve Preserve Preserve Preserve Transfer Functions Preserve UCR and BG Settings Preserve Overprint Settings US Web Coated (SWOP) v2 US Web Coated (SWOP) v2 Color Workspace: CMYK sRGB IEC61966-2.1 sRGB IEC61966-2.1 Color Workspace: RGB Color Workspace: Gray Leave Color Unchanged; Default Intent Leave Color Unchanged; Default Intent Tag Everything for Color Management; Default Intent Convert All Colors to sRGB; Default Intent Color Management Policies Isaacs Press Print eBook

bc 71 Distiller Job Options Compared Standard Distiller & Isaacs Job Options Sets ASCII Format Isaacs 150 effectively Screen PostScript Job Option Override Preserve DSC Document Info Preserve OPI Comments Preserve EPS DSC Information Resize & Center EPS Log DSC Warnings Convert Gradients to Smooth Shades Illustrator Overprint Mode Save PJT in PDF Preserve Level 2 Copypage Isaacs Press Print eBook

bc 72 Cheap Prologue.ps Trick Forcing the Image Interpolation Option The Prologue.ps and Epilogue.ps facility Designed for special effects via custom PostScript run by Distiller at the start and end of each PostScript job Requires detailed knowledge of PostScript to write Prologue.ps and Epilogue.ps code. Examples: Cover page generation Logging of Distiller parameters Global fixup of bum PostScript Not generally recommended for normal use of Acrobat in the enterprise

bc 73 Cheap Prologue.ps Trick Forcing the Image Interpolation Option Useful example of use of Prologue.ps to force the high quality image interpolation option "on" for all images: % Redefine image operator to set Interpolate to true unconditionally. /image { dup type /dicttype eq { dup /Interpolate true put }if //image } bind def

bc Post-PDF File Creation Tweaking & "Other" Considerations

bc 75 Wonderful Plug-Ins for Acrobat For certain functions not built-into Acrobat 5, available third-party plug-ins provide excellent solutions Color and colorspace modifications Advanced touch up Document imposition Separations

bc 76 Wonderful Plug-Ins for Acrobat Quite A Box of Tricks --QuiteSoftware Wide variety of "tricks" More on the color conversion tools, later

bc 77 Wonderful Plug-Ins for Acrobat Quite Imposing and Quite Imposing Plus --QuiteSoftware Booklet creation, n-up pages, step & repeat, as well as general page imposition functions Easy step-by-step "easy imposition" functions Readily usable by office workers for most functions

bc 78 Wonderful Plug-Ins for Acrobat PitStop Professional --EnfocusSoftware Virtual "Swiss Army knife" of tools for analysis and fixup of PDF files Oriented more towards prepress professionals

bc 79 Wonderful Plug-Ins for Acrobat CrackerJack -- Lantana Research Software Corporation Separations for prepress from PDF files within Acrobat Others...

bc 80 Prepress Color Issues CMYK versus RGB NOT an issue for high-end publishing programs that provide for CMYK color and managed color Primarily an issue for typical "enterprise applications" under Windows AND Macintosh that only support RGB color Generally NOT an issue for composite color output devices (i.e., color laser printers, high-end inkjet printers) PostScript does automatic conversion of RGB or CMYK Acrobat printing options for color management Rich black problem under Windows solved by driver TrueGray options (R=G=B text & vector graphics to K)

bc 81 Prepress Color Issues However, for many prepress professionals... It's a CMYK World After All... Solutions in the enterprise... Excellent and inexpensive global RGB to CMYK conversion via Quite A Box of Tricks plugin Use EPS from CMYK-capable applications for precise CMYK (or managed color) as well as spot color where and when necessary PostScript pre-processing programs such as Preflight2000 Colour Chameleon from Grafikhuset

bc 82 Acrobat "Save as EPS" EPS Generation Options Controls the options by which EPS can be exported by Acrobat's "Save as EPS" capability The "defaults" are likely not what you want!

bc Q&A

bcyou lookTM everywhere
